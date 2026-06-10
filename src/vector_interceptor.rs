use crate::vector::VectorIndex;

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

/// Extension trait that adds vector search capabilities to
/// [CustomizableOperation](aws_sdk_dynamodb::client::customize::CustomizableOperation).
pub trait VectorSearchExt<T, E, B> {
    fn vector_indexes(self, indexes: Vec<VectorIndex>) -> Self;
}

impl<T, E, B> VectorSearchExt<T, E, B> for CustomizableOperation<T, E, B> {
    fn vector_indexes(self, indexes: Vec<VectorIndex>) -> Self {
        self.interceptor(VectorRequestStoreInterceptor::for_vector_indexes(indexes))
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
