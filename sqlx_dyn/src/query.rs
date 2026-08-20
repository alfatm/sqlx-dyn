//! Query wrappers returned by the macros.
//!
//! Each type owns a `QueryBuilder` and exposes consuming `fetch_*`/`execute`
//! methods. Consuming is required because `QueryBuilder::build*` borrows the
//! builder mutably for the whole query; an owned `self` inside an `async fn`
//! keeps that borrow valid across the await without exposing a self-referential
//! type to callers.

use sqlx::postgres::{PgQueryResult, PgRow};
use sqlx::{Executor, FromRow, Postgres, QueryBuilder};

/// An untyped dynamic query. Rows come back as [`PgRow`].
pub struct DynQuery {
    builder: QueryBuilder<Postgres>,
}

impl DynQuery {
    pub fn new(builder: QueryBuilder<Postgres>) -> Self {
        Self { builder }
    }

    /// The assembled SQL, for debugging and tests. Bind values are not shown;
    /// they travel out-of-band as parameters.
    pub fn sql(&self) -> String {
        self.builder.sql().as_str().to_owned()
    }

    /// Escape hatch: take the underlying builder to append more SQL.
    pub fn builder_mut(&mut self) -> &mut QueryBuilder<Postgres> {
        &mut self.builder
    }

    pub fn into_builder(self) -> QueryBuilder<Postgres> {
        self.builder
    }

    pub async fn fetch_all<'c, E>(mut self, executor: E) -> sqlx::Result<Vec<PgRow>>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build().fetch_all(executor).await
    }

    pub async fn fetch_one<'c, E>(mut self, executor: E) -> sqlx::Result<PgRow>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build().fetch_one(executor).await
    }

    pub async fn fetch_optional<'c, E>(mut self, executor: E) -> sqlx::Result<Option<PgRow>>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build().fetch_optional(executor).await
    }

    pub async fn execute<'c, E>(mut self, executor: E) -> sqlx::Result<PgQueryResult>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build().execute(executor).await
    }
}

/// A dynamic query decoding rows into `T` via [`FromRow`].
pub struct DynQueryAs<T> {
    builder: QueryBuilder<Postgres>,
    _out: std::marker::PhantomData<T>,
}

/// Construction and inspection without decode bounds: merely looking at the SQL
/// must not require `T` to be decodable.
impl<T> DynQueryAs<T> {
    pub fn new(builder: QueryBuilder<Postgres>) -> Self {
        Self {
            builder,
            _out: std::marker::PhantomData,
        }
    }

    /// The assembled SQL, for debugging and tests.
    pub fn sql(&self) -> String {
        self.builder.sql().as_str().to_owned()
    }

    pub fn builder_mut(&mut self) -> &mut QueryBuilder<Postgres> {
        &mut self.builder
    }
}

impl<T> DynQueryAs<T>
where
    T: Send + Unpin + for<'r> FromRow<'r, PgRow>,
{
    pub async fn fetch_all<'c, E>(mut self, executor: E) -> sqlx::Result<Vec<T>>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build_query_as::<T>().fetch_all(executor).await
    }

    pub async fn fetch_one<'c, E>(mut self, executor: E) -> sqlx::Result<T>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build_query_as::<T>().fetch_one(executor).await
    }

    pub async fn fetch_optional<'c, E>(mut self, executor: E) -> sqlx::Result<Option<T>>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder
            .build_query_as::<T>()
            .fetch_optional(executor)
            .await
    }

    /// sqlx's `QueryAs` has no `execute`, so this drops the decode type and runs
    /// the statement for the row count.
    pub async fn execute<'c, E>(mut self, executor: E) -> sqlx::Result<PgQueryResult>
    where
        E: Executor<'c, Database = Postgres>,
    {
        self.builder.build().execute(executor).await
    }
}

/// A dynamic query returning a single column.
///
/// The scalar type is a parameter of the `fetch_*` methods rather than of the
/// struct, so `query_scalar!(...)` needs no type annotation and `.sql()` is
/// callable on its own. The type is pinned at the fetch call site, as in
/// `sqlx::query_scalar`.
pub struct DynQueryScalar {
    builder: QueryBuilder<Postgres>,
}

impl DynQueryScalar {
    pub fn new(builder: QueryBuilder<Postgres>) -> Self {
        Self { builder }
    }

    /// The assembled SQL, for debugging and tests.
    pub fn sql(&self) -> String {
        self.builder.sql().as_str().to_owned()
    }

    pub fn builder_mut(&mut self) -> &mut QueryBuilder<Postgres> {
        &mut self.builder
    }

    pub async fn fetch_all<'c, T, E>(mut self, executor: E) -> sqlx::Result<Vec<T>>
    where
        E: Executor<'c, Database = Postgres>,
        T: Send + Unpin,
        (T,): Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        self.builder
            .build_query_scalar::<T>()
            .fetch_all(executor)
            .await
    }

    pub async fn fetch_one<'c, T, E>(mut self, executor: E) -> sqlx::Result<T>
    where
        E: Executor<'c, Database = Postgres>,
        T: Send + Unpin,
        (T,): Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        self.builder
            .build_query_scalar::<T>()
            .fetch_one(executor)
            .await
    }

    pub async fn fetch_optional<'c, T, E>(mut self, executor: E) -> sqlx::Result<Option<T>>
    where
        E: Executor<'c, Database = Postgres>,
        T: Send + Unpin,
        (T,): Send + Unpin + for<'r> FromRow<'r, PgRow>,
    {
        self.builder
            .build_query_scalar::<T>()
            .fetch_optional(executor)
            .await
    }
}
