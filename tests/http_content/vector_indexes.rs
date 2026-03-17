//! VectorIndexes Injection Test
//! This test asserts that the driver injects a `VectorIndexes` field into the
//! `CreateTable` JSON body when `.vector_indexes()` is called on the builder.
//! We use a proxy to intercept the CreateTable request before it reaches alternator.

use crate::http_content::http_tester::*;
use crate::http_content::proxy::*;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, Response};

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use alternator_driver::client::Waiters;
use alternator_driver::CreateTableBuilderExt;
use alternator_driver::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};

const TABLE_NAME: &str = "VectorIndexTestTable";
const VECTOR_INDEXES: &[&str] = &["idx1", "idx2"];

// Assert VectorIndexes is present in the CreateTable body, then forward.
async fn on_request(
    request: Request<Incoming>,
    sender: Arc<Mutex<SendRequest<Full<Bytes>>>>,
) -> Response<Full<Bytes>> {
    let (parts, body) = collect_request(request).await;

    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("CreateTable body must be valid JSON");

    let indexes = json
        .get("VectorIndexes")
        .expect("VectorIndexes must be present in CreateTable body");

    let expected: serde_json::Value = serde_json::json!(VECTOR_INDEXES);
    assert_eq!(
        indexes, &expected,
        "VectorIndexes in CreateTable body does not match expected value"
    );

    let (parts, body) = collect_received_response(parts, body, sender).await;
    build_response(parts, body)
}

async fn cleanup(alternator_address: String) {
    let client = alternator_driver::Client::from_conf(
        alternator_driver::Config::builder()
            .endpoint_url(format!("http://{}", alternator_address))
            .credentials_provider(
                alternator_driver::config::Credentials::for_tests_with_session_token(),
            )
            .region(alternator_driver::config::Region::new("eu-central-1"))
            .behavior_version(alternator_driver::config::BehaviorVersion::latest())
            .build(),
    );

    let result = client.delete_table().table_name(TABLE_NAME).send().await;

    if let Err(e) = result {
        if !e
            .as_service_error()
            .is_some_and(|s| s.is_resource_not_found_exception())
        {
            std::panic::panic_any(e);
        }
    } else {
        client
            .wait_until_table_not_exists()
            .table_name(TABLE_NAME)
            .wait(Duration::from_secs(30))
            .await
            .unwrap();
    }
}

#[tokio::test]
pub async fn test() {
    let mut tester = HttpTester::start("localhost:8000".to_string(), cleanup).await;

    let client = alternator_driver::Client::from_conf(
        alternator_driver::Config::builder()
            .endpoint_url(format!("http://{}", tester.get_proxy_address()))
            .credentials_provider(
                alternator_driver::config::Credentials::for_tests_with_session_token(),
            )
            .region(alternator_driver::config::Region::new("eu-central-1"))
            .behavior_version(alternator_driver::config::BehaviorVersion::latest())
            .build(),
    );

    // create table with vector indexes — proxy asserts VectorIndexes is in the body
    tester
        .call_with_proxy(
            async {
                client
                    .create_table()
                    .table_name(TABLE_NAME)
                    .attribute_definitions(
                        AttributeDefinition::builder()
                            .attribute_name("PK")
                            .attribute_type(ScalarAttributeType::S)
                            .build()
                            .unwrap(),
                    )
                    .key_schema(
                        KeySchemaElement::builder()
                            .attribute_name("PK")
                            .key_type(KeyType::Hash)
                            .build()
                            .unwrap(),
                    )
                    .billing_mode(BillingMode::PayPerRequest)
                    .vector_indexes(VECTOR_INDEXES.iter().map(|s| s.to_string()).collect())
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    tester.finish().await;
}
