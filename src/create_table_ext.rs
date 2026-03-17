use crate::interceptors::VectorIndexesInterceptor;
use aws_sdk_dynamodb::operation::create_table::{CreateTableError, CreateTableOutput};
use aws_sdk_dynamodb::operation::create_table::builders::CreateTableFluentBuilder;
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use aws_smithy_runtime_api::client::result::SdkError;

/// A `CreateTable` builder that carries a list of vector index names.
/// Obtained by calling [`CreateTableBuilderExt::vector_indexes`] on a
/// [`CreateTableFluentBuilder`].  Call `.send().await` to dispatch the request.
pub struct CreateTableWithVectorIndexes {
    inner: CreateTableFluentBuilder,
    vector_indexes: Vec<String>,
}

impl CreateTableWithVectorIndexes {
    /// Send the `CreateTable` request, injecting `VectorIndexes` into the body.
    pub async fn send(
        self,
    ) -> Result<CreateTableOutput, SdkError<CreateTableError, HttpResponse>> {
        let interceptor = VectorIndexesInterceptor {
            vector_indexes: self.vector_indexes,
        };
        self.inner.customize().interceptor(interceptor).send().await
    }
}

/// Extension trait that adds `.vector_indexes()` to [`CreateTableFluentBuilder`].
///
/// # Example
/// ```ignore
/// use alternator_driver::CreateTableBuilderExt;
///
/// client.create_table()
///     .table_name("MyTable")
///     // ... attribute_definitions, key_schema, etc. ...
///     .vector_indexes(vec!["idx1".to_string()])
///     .send()
///     .await?;
/// ```
pub trait CreateTableBuilderExt {
    /// Consume the builder and attach a list of vector index names.
    /// Call `.send().await` on the returned value to dispatch.
    fn vector_indexes(self, indexes: Vec<String>) -> CreateTableWithVectorIndexes;
}

impl CreateTableBuilderExt for CreateTableFluentBuilder {
    fn vector_indexes(self, indexes: Vec<String>) -> CreateTableWithVectorIndexes {
        CreateTableWithVectorIndexes {
            inner: self,
            vector_indexes: indexes,
        }
    }
}
