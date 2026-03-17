use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextMut;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::config_bag::ConfigBag;

/// Injects a `VectorIndexes` field into the JSON body of a CreateTable request
/// before it is signed and transmitted.
#[derive(Debug)]
pub struct VectorIndexesInterceptor {
    pub vector_indexes: Vec<String>,
}

impl Intercept for VectorIndexesInterceptor {
    fn name(&self) -> &'static str {
        "VectorIndexesInterceptor"
    }

    fn modify_before_signing(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let request = context.request_mut();
        let body_bytes = request
            .body()
            .bytes()
            .expect("DynamoDB CreateTable body must be fully buffered")
            .to_vec();

        let mut json: serde_json::Value = serde_json::from_slice(&body_bytes)?;
        if let Some(obj) = json.as_object_mut() {
            obj.insert(
                "VectorIndexes".to_string(),
                serde_json::json!(&self.vector_indexes),
            );
        }

        let new_bytes = serde_json::to_vec(&json)?;
        // Keep Content-Length accurate so receivers don't truncate the body.
        request.headers_mut().insert(
            "content-length",
            new_bytes.len().to_string(),
        );
        *request.body_mut() = SdkBody::from(new_bytes);
        Ok(())
    }
}
