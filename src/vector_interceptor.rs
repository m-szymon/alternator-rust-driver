use crate::vector::{VectorIndex, VectorIndexUpdate, VectorSearch};

use aws_sdk_dynamodb::client::customize::CustomizableOperation;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::interceptors::context::BeforeSerializationInterceptorContextMut;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::{ConfigBag, Storable, StoreReplace};

/// State carried through [ConfigBag] describing vector-search request
/// extras that [crate::AlternatorInterceptor] should inject into the
/// serialized JSON body immediately before compression.
///
/// This is populated by [VectorRequestStoreInterceptor] in
/// `modify_before_serialization` and is retry-safe: it never inspects or
/// mutates the HTTP body itself, only carries caller intent.
#[derive(Debug, Clone, Default)]
pub(crate) struct VectorRequestStore {
    pub(crate) vector_indexes: Option<Vec<VectorIndex>>,
    pub(crate) vector_index_updates: Option<Vec<VectorIndexUpdate>>,
    pub(crate) vector_search: Option<VectorSearch>,
}

impl Storable for VectorRequestStore {
    type Storer = StoreReplace<Self>;
}

/// Per-operation interceptor that stashes vector-search request state into
/// [ConfigBag], to be picked up later by [crate::AlternatorInterceptor]
/// before compression.
#[derive(Debug, Clone)]
pub(crate) struct VectorRequestStoreInterceptor {
    store: VectorRequestStore,
}

impl VectorRequestStoreInterceptor {
    pub(crate) fn for_vector_indexes(vector_indexes: Vec<VectorIndex>) -> Self {
        Self {
            store: VectorRequestStore {
                vector_indexes: Some(vector_indexes),
                ..Default::default()
            },
        }
    }

    pub(crate) fn for_vector_index_updates(updates: Vec<VectorIndexUpdate>) -> Self {
        Self {
            store: VectorRequestStore {
                vector_index_updates: Some(updates),
                ..Default::default()
            },
        }
    }

    pub(crate) fn for_vector_search(search: VectorSearch) -> Self {
        Self {
            store: VectorRequestStore {
                vector_search: Some(search),
                ..Default::default()
            },
        }
    }
}

impl Intercept for VectorRequestStoreInterceptor {
    fn name(&self) -> &'static str {
        "VectorRequestStoreInterceptor"
    }

    fn modify_before_serialization(
        &self,
        _: &mut BeforeSerializationInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        cfg.interceptor_state().store_put(self.store.clone());

        Ok(())
    }
}

/// Extension trait that adds `VectorIndexes` support to CreateTable's
/// [CustomizableOperation](aws_sdk_dynamodb::client::customize::CustomizableOperation).
///
/// Implemented only for the generated CreateTable customizable operation, so
/// misuse on other operations is a compile error rather than a runtime one.
pub trait CreateTableVectorExt {
    fn vector_indexes(self, indexes: Vec<VectorIndex>) -> Self;
}

impl<E, B> CreateTableVectorExt
    for CustomizableOperation<aws_sdk_dynamodb::operation::create_table::CreateTableOutput, E, B>
{
    fn vector_indexes(self, indexes: Vec<VectorIndex>) -> Self {
        self.interceptor(VectorRequestStoreInterceptor::for_vector_indexes(indexes))
    }
}

/// Extension trait that adds `VectorIndexUpdates` support to UpdateTable's
/// [CustomizableOperation](aws_sdk_dynamodb::client::customize::CustomizableOperation).
///
/// Implemented only for the generated UpdateTable customizable operation, so
/// misuse on other operations is a compile error rather than a runtime one:
///
/// ```compile_fail
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use alternator_driver::{AlternatorClient, AlternatorConfig, UpdateTableVectorExt, VectorIndexUpdate};
///
/// let client = AlternatorClient::from_conf(
///     AlternatorConfig::builder().behavior_version_latest().build(),
/// );
///
/// // `vector_index_updates` is only defined for UpdateTable, not CreateTable.
/// let _ = client
///     .create_table()
///     .table_name("t")
///     .customize()
///     .vector_index_updates(Vec::<VectorIndexUpdate>::new());
/// # });
/// ```
pub trait UpdateTableVectorExt {
    fn vector_index_updates(self, updates: Vec<VectorIndexUpdate>) -> Self;
}

impl<E, B> UpdateTableVectorExt
    for CustomizableOperation<aws_sdk_dynamodb::operation::update_table::UpdateTableOutput, E, B>
{
    fn vector_index_updates(self, updates: Vec<VectorIndexUpdate>) -> Self {
        self.interceptor(VectorRequestStoreInterceptor::for_vector_index_updates(
            updates,
        ))
    }
}

/// Extension trait that adds `VectorSearch` support to Query's
/// [CustomizableOperation](aws_sdk_dynamodb::client::customize::CustomizableOperation).
///
/// Implemented only for the generated Query customizable operation, so
/// misuse on other operations is a compile error rather than a runtime one.
pub trait QueryVectorExt {
    fn vector_search(self, search: VectorSearch) -> Self;
}

impl<E, B> QueryVectorExt
    for CustomizableOperation<aws_sdk_dynamodb::operation::query::QueryOutput, E, B>
{
    fn vector_search(self, search: VectorSearch) -> Self {
        self.interceptor(VectorRequestStoreInterceptor::for_vector_search(search))
    }
}

#[cfg(test)]
mod tests {
    use crate::vector::{VectorAttribute, VectorIndexJson};

    /// Test that our vector index serialization produces correct JSON
    /// that the AlternatorInterceptor would inject into the CreateTable body.
    #[test]
    fn test_vector_index_json_structure() {
        let va = VectorAttribute::builder()
            .attribute_name("embedding")
            .dimensions(128)
            .build()
            .unwrap();

        let idx = crate::vector::VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .build()
            .unwrap();

        let json_idx: VectorIndexJson = (&idx).into();
        let value = serde_json::to_value(&json_idx).unwrap();

        assert_eq!(value["IndexName"], "vec_idx");
        // No KeySchema — automatically inherits from base table
        assert!(value.get("KeySchema").is_none());
        // No projection configured, so the field is omitted entirely.
        assert!(value.get("Projection").is_none());
        assert_eq!(value["VectorAttribute"]["AttributeName"], "embedding");
        assert_eq!(value["VectorAttribute"]["Dimensions"], 128);
    }
}
