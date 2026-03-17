//! Correct Request Line Test
//! This test asserts driver generates only requests with correct line: Method = POST, URI = "/".
//! We use a proxy to intercept messages sent between driver and alternator (see HttpTester).

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
use alternator_driver::types::{
    AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
    ScalarAttributeType,
};

// forward requests, assert correct line: POST /
async fn on_request(
    request: Request<Incoming>,
    sender: Arc<Mutex<SendRequest<Full<Bytes>>>>,
) -> Response<Full<Bytes>> {
    let (parts, body) = collect_request(request).await;

    // check HTTP line correctness
    assert_eq!(
        parts.method.as_str(),
        "POST",
        "Unexpected HTTP request method"
    );
    assert_eq!(
        parts.uri,
        http::Uri::from_static("/"),
        "Unexpected HTTP request URI"
    );

    // forward
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

    let result = client
        .delete_table()
        .table_name("CorrectLineExampleTable")
        .send()
        .await;

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
            .table_name("CorrectLineExampleTable")
            .wait(Duration::from_secs(1))
            .await
            .unwrap();
    }
}

#[tokio::test]
pub async fn test() {
    // start the test with default cleanup database function
    let mut tester = HttpTester::start("localhost:8000".to_string(), cleanup).await;

    // construct client
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

    // perform calls to alternator, use proxy to peek and forward requests

    // create table
    tester
        .call_with_proxy(
            async {
                client
                    .create_table()
                    .table_name("CorrectLineExampleTable")
                    .attribute_definitions(
                        AttributeDefinition::builder()
                            .attribute_name("ExampleKey")
                            .attribute_type(ScalarAttributeType::S)
                            .build()
                            .unwrap(),
                    )
                    .key_schema(
                        KeySchemaElement::builder()
                            .attribute_name("ExampleKey")
                            .key_type(KeyType::Hash)
                            .build()
                            .unwrap(),
                    )
                    .billing_mode(BillingMode::PayPerRequest)
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // wait for table
    tester
        .call_with_proxy(
            async {
                client
                    .wait_until_table_exists()
                    .table_name("CorrectLineExampleTable")
                    .wait(Duration::from_secs(1))
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // put item
    tester
        .call_with_proxy(
            async {
                client
                    .put_item()
                    .table_name("CorrectLineExampleTable")
                    .item(
                        "ExampleKey",
                        AttributeValue::S("ExampleItemKey".to_string()),
                    )
                    .item(
                        "ExampleAttribute",
                        AttributeValue::S("ExampleItem".to_string()),
                    )
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // update item
    tester
        .call_with_proxy(
            async {
                client
                    .update_item()
                    .table_name("CorrectLineExampleTable")
                    .key(
                        "ExampleKey",
                        AttributeValue::S("ExampleItemKey".to_string()),
                    )
                    .update_expression("SET #d = :v")
                    .expression_attribute_names("#d", "ExampleAttribute")
                    .expression_attribute_values(
                        ":v",
                        AttributeValue::S("ExampleItemUpdated".to_string()),
                    )
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // get item
    tester
        .call_with_proxy(
            async {
                client
                    .get_item()
                    .table_name("CorrectLineExampleTable")
                    .key(
                        "ExampleKey",
                        AttributeValue::S("ExampleItemKey".to_string()),
                    )
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // delete table
    tester
        .call_with_proxy(
            async {
                client
                    .delete_table()
                    .table_name("CorrectLineExampleTable")
                    .send()
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // wait for table
    tester
        .call_with_proxy(
            async {
                client
                    .wait_until_table_not_exists()
                    .table_name("CorrectLineExampleTable")
                    .wait(Duration::from_secs(1))
                    .await
                    .unwrap();
            },
            on_request,
        )
        .await;

    // cleanup
    tester.finish().await;
}
