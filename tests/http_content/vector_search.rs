use crate::http_content::driver_utils::*;
use crate::http_content::http_test::*;
use crate::http_content::proxy::*;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, Response};

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use std::sync::Arc;
use test_context::test_context;
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use alternator_driver::*;

async fn cleanup_calls(resources: Vec<String>, alternator_address: &str) {
    let client = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::Config::builder()
            .endpoint_url(format!("http://{}", alternator_address))
            .region(aws_sdk_dynamodb::config::Region::new("eu-central-1"))
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .credentials_provider(
                aws_sdk_dynamodb::config::Credentials::for_tests_with_session_token(),
            )
            .build(),
    );

    for resource in resources {
        delete_table_cleanup(&client, &resource).await;
    }
}

struct VectorSearchConfig;

impl HttpTestConfig for VectorSearchConfig {
    async fn on_request(
        request: Request<Incoming>,
        sender: Arc<TokioMutex<SendRequest<Full<Bytes>>>>,
    ) -> Response<Full<Bytes>> {
        let (parts, body) = collect_request(request).await;

        let is_create_table = parts
            .headers
            .get("x-amz-target")
            .map(|v| v.as_bytes() == b"DynamoDB_20120810.CreateTable")
            .unwrap_or(false);

        if is_create_table {
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

            assert!(
                json.get("VectorIndexes").is_some(),
                "VectorIndexes should be present in CreateTable request"
            );

            let indexes = json["VectorIndexes"].as_array().unwrap();
            assert_eq!(indexes.len(), 1, "Should have 1 vector index");
            assert_eq!(indexes[0]["IndexName"], "vec_idx");
            assert_eq!(
                indexes[0]["VectorAttribute"]["AttributeName"],
                "vector_attr"
            );
            // VectorAttribute has no VectorType field in the API
            assert_eq!(indexes[0]["VectorAttribute"]["Dimensions"], 128);

            // Strip VectorIndexes before forwarding to Alternator
            let mut stripped = json.clone();
            stripped.as_object_mut().unwrap().remove("VectorIndexes");
            let new_body = serde_json::to_vec(&stripped).unwrap();

            let mut mod_parts = parts.clone();
            mod_parts.headers.insert(
                "content-length",
                new_body.len().to_string().try_into().unwrap(),
            );

            let (parts, body) =
                collect_received_response(mod_parts, Bytes::from(new_body), sender).await;
            build_response(parts, body)
        } else {
            let (parts, body) = collect_received_response(parts, body, sender).await;
            build_response(parts, body)
        }
    }

    async fn cleanup(resources: Vec<String>, alternator_address: &str) {
        cleanup_calls(resources, alternator_address).await;
    }
}

#[test_context(HttpTestContext<VectorSearchConfig>)]
#[tokio::test]
pub async fn test_create_table_with_vector_indexes(ctx: &mut HttpTestContext<VectorSearchConfig>) {
    let client = AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .build(),
    );

    let table_name = format!("table_vec_{}", Uuid::new_v4());
    ctx.register_resource(table_name.clone());

    let va = VectorAttribute::builder()
        .attribute_name("vector_attr")
        .dimensions(128)
        .build()
        .unwrap();

    let vi = VectorIndex::builder()
        .index_name("vec_idx")
        .vector_attribute(va)
        .build()
        .unwrap();

    let created = client
        .create_table()
        .table_name(&table_name)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .billing_mode(BillingMode::PayPerRequest)
        .customize()
        .vector_indexes(vec![vi])
        .send()
        .await
        .unwrap();

    assert!(created.table_description().is_some());

    // Verify table was actually created by performing DescribeTable
    let desc = client
        .describe_table()
        .table_name(&table_name)
        .send()
        .await
        .unwrap();

    assert_eq!(desc.table().unwrap().table_name().unwrap(), &table_name);

    // Cleanup is done by teardown
}
