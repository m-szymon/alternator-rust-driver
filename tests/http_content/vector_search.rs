// Copyright ScyllaDB, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! HTTP-level coverage of the vector-search related behaviour of the client:
//! the generated AWS SDK's native `VectorIndexes`/`SearchVectors` shapes
//! pass through unchanged, Alternator's `SearchVectors` extensions
//! (`BaseRead`, `FilterExpression`) are injected, and `FLOAT32VECTOR`
//! values are rewritten on the way out and converted on the way in.

use crate::http_content::driver_utils::*;
use crate::http_content::http_test::*;
use crate::http_content::proxy::*;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, Response, StatusCode};

use aws_sdk_dynamodb::error::ProvideErrorMetadata;
use aws_sdk_dynamodb::primitives::Blob;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, IndexStatus, KeySchemaElement, KeyType,
    Projection, ProjectionType, ScalarAttributeType, VectorAttributeDefinition,
    VectorDistanceFunction, VectorIndex,
};
use flate2::read::GzDecoder;
use std::io::Read;
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

fn sample_index() -> VectorIndex {
    VectorIndex::builder()
        .index_name("vec_idx")
        .vector_attribute(
            VectorAttributeDefinition::builder()
                .attribute_name("vector_attr")
                .build()
                .unwrap(),
        )
        .dimensions(128)
        .distance_function(VectorDistanceFunction::Cosine)
        .projection(
            Projection::builder()
                .projection_type(ProjectionType::KeysOnly)
                .build(),
        )
        .build()
        .unwrap()
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

            // The generated SDK serializes VectorIndexes in exactly the shape
            // Alternator's DynamoDB-compatible API documents.
            let indexes = json["VectorIndexes"]
                .as_array()
                .expect("VectorIndexes should be present in CreateTable request");
            assert_eq!(indexes.len(), 1, "Should have 1 vector index");
            assert_eq!(indexes[0]["IndexName"], "vec_idx");
            assert_eq!(
                indexes[0]["VectorAttribute"],
                serde_json::json!({ "AttributeName": "vector_attr" })
            );
            assert_eq!(indexes[0]["Dimensions"], 128);
            assert_eq!(indexes[0]["DistanceFunction"], "COSINE");
            assert_eq!(indexes[0]["Projection"]["ProjectionType"], "KEYS_ONLY");

            // Strip VectorIndexes before forwarding to a backend that may
            // not have a Vector Store attached.
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
        .vector_indexes(sample_index())
        .send()
        .await
        .unwrap();
    assert!(created.table_description().is_some());

    // The forwarding proxy strips VectorIndexes, so only ordinary table
    // metadata is available from this real-Scylla request.
    let desc = client
        .describe_table()
        .table_name(&table_name)
        .send()
        .await
        .unwrap();

    assert_eq!(desc.table().unwrap().table_name().unwrap(), &table_name);

    // Cleanup is done by teardown
}

/// Config used by the tests below: instead of forwarding to the real
/// backend, it returns a caller-supplied synthetic 200 JSON response and
/// captures the last request body, both stored per-test via
/// [HttpTestContext::set_on_request]. This lets us assert on HTTP JSON
/// content and client-side response decoding without depending on the
/// backend supporting vector search.
struct SyntheticResponseConfig;

impl HttpTestConfig for SyntheticResponseConfig {
    async fn on_request(
        request: Request<Incoming>,
        _sender: Arc<TokioMutex<SendRequest<Full<Bytes>>>>,
    ) -> Response<Full<Bytes>> {
        let (_parts, _body) = collect_request(request).await;
        Response::builder()
            .status(200)
            .header("content-type", "application/x-amz-json-1.0")
            .body(Full::new(Bytes::from(b"{}".to_vec())))
            .unwrap()
    }

    async fn cleanup(_resources: Vec<String>, _alternator_address: &str) {}
}

/// Sets up `ctx`'s `on_request` hook to record every request's (possibly
/// gzip-compressed) raw body and headers into `last_request`, and reply with
/// `response_body`, for the current test only.
async fn capture_raw_and_respond(
    ctx: &HttpTestContext<SyntheticResponseConfig>,
    response_body: serde_json::Value,
    last_request: Arc<TokioMutex<Option<(http::HeaderMap, Bytes)>>>,
) {
    let response_bytes = serde_json::to_vec(&response_body).unwrap();
    ctx.set_on_request(move |request, _sender| {
        let last_request = last_request.clone();
        let response_bytes = response_bytes.clone();
        async move {
            let (parts, body) = collect_request(request).await;
            *last_request.lock().await = Some((parts.headers.clone(), body));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/x-amz-json-1.0")
                .body(Full::new(Bytes::from(response_bytes)))
                .unwrap()
        }
    })
    .await;
}

/// Sets up `ctx`'s `on_request` hook to record every request body into
/// `last_body` and reply with `response_body`, for the current test only.
async fn capture_and_respond(
    ctx: &HttpTestContext<SyntheticResponseConfig>,
    status: StatusCode,
    response_body: serde_json::Value,
    last_body: Arc<TokioMutex<Option<serde_json::Value>>>,
) {
    let response_bytes = serde_json::to_vec(&response_body).unwrap();
    ctx.set_on_request(move |request, _sender| {
        let last_body = last_body.clone();
        let response_bytes = response_bytes.clone();
        async move {
            let (_parts, body) = collect_request(request).await;
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) {
                *last_body.lock().await = Some(json);
            }
            Response::builder()
                .status(status)
                .header("content-type", "application/x-amz-json-1.0")
                .body(Full::new(Bytes::from(response_bytes)))
                .unwrap()
        }
    })
    .await;
}

fn synthetic_client(ctx: &HttpTestContext<SyntheticResponseConfig>) -> AlternatorClient {
    AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .build(),
    )
}

fn search_results_response() -> serde_json::Value {
    serde_json::json!({
        "SearchResults": [
            {
                "Item": {
                    "pk": { "S": "a" },
                    "embedding": { "FLOAT32VECTOR": [1.0, 0.0, 0.0] }
                },
                "Score": 0.0
            },
            {
                "Item": { "pk": { "S": "b" } },
                "Score": 0.42
            }
        ]
    })
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_search_vectors_sends_extensions_and_float32vector_search_vector(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        search_results_response(),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    let output = client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(Float32Vector::to_attribute_value([1.0, 2.0, 3.0]).unwrap())
        .top_k(10)
        .expression_attribute_names("#l", "lang")
        .expression_attribute_values(":l", AttributeValue::S("en".into()))
        .base_read(true)
        .filter_expression("#l = :l")
        .send()
        .await
        .unwrap();

    let body = last_body.lock().await.take().unwrap();
    assert_eq!(body["TableName"], "some_table");
    assert_eq!(body["IndexName"], "embedding_idx");
    assert_eq!(body["TopK"], 10);
    assert_eq!(
        body["SearchVector"],
        serde_json::json!({ "FLOAT32VECTOR": [1.0, 2.0, 3.0] })
    );
    assert_eq!(body["BaseRead"], true);
    assert_eq!(body["FilterExpression"], "#l = :l");
    assert_eq!(body["ExpressionAttributeNames"]["#l"], "lang");
    assert_eq!(body["ExpressionAttributeValues"][":l"]["S"], "en");

    // The generated output is used as-is: ordered results with scores, and
    // FLOAT32VECTOR item attributes converted to L/N by default.
    let results = output.search_results();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].score(), 0.0);
    assert_eq!(results[1].score(), 0.42);
    assert_eq!(
        results[0].item().unwrap()["embedding"],
        AttributeValue::L(vec![
            AttributeValue::N("1".into()),
            AttributeValue::N("0".into()),
            AttributeValue::N("0".into()),
        ])
    );
    assert_eq!(
        results[1].item().unwrap()["pk"],
        AttributeValue::S("b".into())
    );
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_search_vectors_customize_form_coexists_with_config_override(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        search_results_response(),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    // The `.customize()` form is equivalent to the direct form, and can be
    // combined with a per-request `alternator_config_override(...)`.
    let output = client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(AttributeValue::N("1".into()))
        .top_k(10)
        .customize()
        .base_read(false)
        .alternator_config_override(
            AlternatorConfig::operation_builder().preserve_float32_vectors(true),
        )
        .send()
        .await
        .unwrap();

    let body = last_body.lock().await.take().unwrap();
    assert_eq!(body["BaseRead"], false);
    assert!(body.get("FilterExpression").is_none());
    assert_eq!(body["SearchVector"], serde_json::json!([{ "N": "1" }]));

    let embedding = &output.search_results()[0].item().unwrap()["embedding"];
    assert!(embedding.is_float32_vector());
    assert_eq!(embedding.float32_vector().unwrap(), vec![1.0, 0.0, 0.0]);
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_ordinary_search_vectors_remains_aws_sdk_compatible(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({ "SearchResults": [] }),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    // Without any extension call, the request is exactly what the AWS SDK
    // would send on its own.
    let output = client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(AttributeValue::N("1".into()))
        .search_vector(AttributeValue::N("2".into()))
        .top_k(3)
        .send()
        .await
        .unwrap();
    assert!(output.search_results().is_empty());

    let body = last_body.lock().await.take().unwrap();
    assert!(body.get("BaseRead").is_none());
    assert!(body.get("FilterExpression").is_none());
    assert_eq!(
        body["SearchVector"],
        serde_json::json!([{ "N": "1" }, { "N": "2" }])
    );
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_search_vectors_preserves_service_errors(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    let error = serde_json::json!({
        "__type": "com.amazonaws.dynamodb.v20120810#ValidationException",
        "message": "Vector index is not ready",
        "Item": { "embedding": { "FLOAT32VECTOR": [1.0] } }
    });
    capture_and_respond(ctx, StatusCode::BAD_REQUEST, error, last_body).await;
    let client = synthetic_client(ctx);

    let search_error = client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(Float32Vector::to_attribute_value([1.0]).unwrap())
        .top_k(10)
        .base_read(true)
        .send()
        .await
        .unwrap_err();
    let service_error = search_error.as_service_error().unwrap();
    assert_eq!(service_error.code(), Some("ValidationException"));
    assert_eq!(service_error.message(), Some("Vector index is not ready"));
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_describe_table_parses_vector_indexes_natively(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({
            "Table": {
                "TableName": "some_table",
                "VectorIndexes": [{
                    "IndexName": "vec_idx",
                    "IndexArn": "arn:scylla:alternator:::table/some_table/index/vec_idx",
                    "VectorAttribute": { "AttributeName": "embedding" },
                    "Dimensions": 3,
                    "DistanceFunction": "COSINE",
                    "Projection": { "ProjectionType": "INCLUDE", "NonKeyAttributes": ["title"] },
                    "SearchSchema": [
                        { "AttributeName": "category", "SearchSchemaElementType": "HASH" }
                    ],
                    "IndexStatus": "CREATING",
                    "Backfilling": true
                }]
            }
        }),
        last_body,
    )
    .await;
    let client = synthetic_client(ctx);

    let described = client
        .describe_table()
        .table_name("some_table")
        .send()
        .await
        .unwrap();
    let indexes = described.table().unwrap().vector_indexes();
    assert_eq!(indexes.len(), 1);
    let index = &indexes[0];
    assert_eq!(index.index_name(), Some("vec_idx"));
    assert!(index.index_arn().is_some());
    assert_eq!(
        index.vector_attribute().map(|a| a.attribute_name()),
        Some("embedding")
    );
    assert_eq!(index.dimensions(), Some(3));
    assert_eq!(
        index.distance_function(),
        Some(&VectorDistanceFunction::Cosine)
    );
    assert_eq!(
        index.projection().and_then(|p| p.projection_type()),
        Some(&ProjectionType::Include)
    );
    assert_eq!(index.search_schema().len(), 1);
    assert_eq!(index.index_status(), Some(&IndexStatus::Creating));
    assert_eq!(index.backfilling(), Some(true));
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_preserve_float32_vectors_operation_override_takes_precedence(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({
            "Item": {
                "pk": { "S": "row1" },
                "embedding": { "FLOAT32VECTOR": [1.0, 2.0, 3.0] }
            }
        }),
        last_body,
    )
    .await;

    // Client default is `false` (ordinary L/N conversion); a per-operation
    // override of `true` should take precedence and return a marker binary.
    let client = AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .preserve_float32_vectors(false)
            .build(),
    );

    let output = client
        .get_item()
        .table_name("some_table")
        .key("pk", AttributeValue::S("row1".into()))
        .customize()
        .alternator_config_override(
            AlternatorConfig::operation_builder().preserve_float32_vectors(true),
        )
        .send()
        .await
        .unwrap();

    let item = output.item().unwrap();
    let embedding = item.get("embedding").unwrap();
    assert!(embedding.is_float32_vector());
    assert_eq!(embedding.float32_vector().unwrap(), vec![1.0, 2.0, 3.0]);
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_preserve_float32_vectors_operation_override_false_beats_client_true(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({
            "Item": {
                "pk": { "S": "row1" },
                "embedding": { "FLOAT32VECTOR": [1.0, 2.0, 3.0] }
            }
        }),
        last_body,
    )
    .await;

    // Client default is `true`; a per-operation override of `false` should
    // take precedence and return an ordinary L/N representation.
    let client = AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .preserve_float32_vectors(true)
            .build(),
    );

    let output = client
        .get_item()
        .table_name("some_table")
        .key("pk", AttributeValue::S("row1".into()))
        .customize()
        .alternator_config_override(
            AlternatorConfig::operation_builder().preserve_float32_vectors(false),
        )
        .send()
        .await
        .unwrap();

    let item = output.item().unwrap();
    let embedding = item.get("embedding").unwrap();
    assert!(matches!(embedding, AttributeValue::L(_)));
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_put_item_rewrites_marker_binary_to_float32vector(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({}),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    let av = Float32Vector::to_attribute_value(vec![1.0, 2.0, 3.0]).unwrap();

    client
        .put_item()
        .table_name("some_table")
        .item("pk", AttributeValue::S("row1".into()))
        .item("embedding", av)
        .send()
        .await
        .unwrap();

    let body = last_body.lock().await.take().unwrap();
    assert_eq!(
        body["Item"]["embedding"]["FLOAT32VECTOR"],
        serde_json::json!([1.0, 2.0, 3.0])
    );
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_get_item_response_converts_float32vector_to_l_n_by_default(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({
            "Item": {
                "pk": { "S": "row1" },
                "embedding": { "FLOAT32VECTOR": [1.0, 2.0, 3.0] }
            }
        }),
        last_body,
    )
    .await;
    let client = synthetic_client(ctx);

    let output = client
        .get_item()
        .table_name("some_table")
        .key("pk", AttributeValue::S("row1".into()))
        .send()
        .await
        .unwrap();

    let item = output.item().unwrap();
    let embedding = item.get("embedding").unwrap();
    assert_eq!(
        embedding,
        &AttributeValue::L(vec![
            AttributeValue::N("1".into()),
            AttributeValue::N("2".into()),
            AttributeValue::N("3".into()),
        ])
    );
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_get_item_response_preserves_marker_binary_when_enabled(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({
            "Item": {
                "pk": { "S": "row1" },
                "embedding": { "FLOAT32VECTOR": [1.0, 2.0, 3.0] }
            }
        }),
        last_body,
    )
    .await;

    let client = AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .preserve_float32_vectors(true)
            .build(),
    );

    let output = client
        .get_item()
        .table_name("some_table")
        .key("pk", AttributeValue::S("row1".into()))
        .send()
        .await
        .unwrap();

    let item = output.item().unwrap();
    let embedding = item.get("embedding").unwrap();
    assert!(embedding.is_float32_vector());
    assert_eq!(embedding.float32_vector().unwrap(), vec![1.0, 2.0, 3.0]);
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_search_vectors_extensions_are_injected_before_compression(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_request = Arc::new(TokioMutex::new(None));
    capture_raw_and_respond(
        ctx,
        serde_json::json!({ "SearchResults": [] }),
        last_request.clone(),
    )
    .await;

    let client = AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(format!("http://{}", ctx.get_proxy_address()))
            .seed_hosts(Vec::<String>::new())
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .allow_no_auth()
            .request_compression(RequestCompression::enabled(
                CompressionAlgorithm::Gzip,
                CompressionLevel::default(),
                0,
            ))
            .build(),
    );

    client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(Float32Vector::to_attribute_value([1.0, 2.0]).unwrap())
        .top_k(5)
        .base_read(true)
        .filter_expression("lang = :lang")
        .send()
        .await
        .unwrap();

    let (headers, body) = last_request.lock().await.take().unwrap();
    assert_eq!(
        headers.get("content-encoding").unwrap(),
        "gzip",
        "request compression must still apply"
    );

    let mut decompressed = Vec::new();
    GzDecoder::new(body.as_ref())
        .read_to_end(&mut decompressed)
        .expect("valid gzip body");
    let json: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();

    // The extension and FLOAT32VECTOR rewrites happened *before* compression:
    // both are visible in the decompressed body.
    assert_eq!(json["BaseRead"], true);
    assert_eq!(json["FilterExpression"], "lang = :lang");
    assert_eq!(
        json["SearchVector"],
        serde_json::json!({ "FLOAT32VECTOR": [1.0, 2.0] })
    );
    assert_eq!(json["TopK"], 5);
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_search_vectors_rejects_marker_mixed_with_other_elements(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({ "SearchResults": [] }),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    // A FLOAT32VECTOR marker is the *whole* search vector or nothing: mixing
    // it with ordinary N elements is rejected locally, before any request.
    let error = client
        .search_vectors()
        .table_name("some_table")
        .index_name("embedding_idx")
        .search_vector(Float32Vector::to_attribute_value([1.0]).unwrap())
        .search_vector(AttributeValue::N("2".into()))
        .top_k(5)
        .send()
        .await
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("exactly one"),
        "unexpected error: {error:?}"
    );
    assert!(
        last_body.lock().await.is_none(),
        "no request should reach the server"
    );
}

#[test_context(HttpTestContext<SyntheticResponseConfig>)]
#[tokio::test]
pub async fn test_ordinary_binary_values_are_left_unchanged(
    ctx: &mut HttpTestContext<SyntheticResponseConfig>,
) {
    let last_body = Arc::new(TokioMutex::new(None));
    capture_and_respond(
        ctx,
        StatusCode::OK,
        serde_json::json!({}),
        last_body.clone(),
    )
    .await;
    let client = synthetic_client(ctx);

    // A binary value that is not a marker must never be mistaken for a vector.
    client
        .put_item()
        .table_name("some_table")
        .item("pk", AttributeValue::S("row1".into()))
        .item("blob", AttributeValue::B(Blob::new(vec![1, 2, 3, 4])))
        .send()
        .await
        .unwrap();

    let body = last_body.lock().await.take().unwrap();
    assert!(body["Item"]["blob"].get("FLOAT32VECTOR").is_none());
    assert!(body["Item"]["blob"].get("B").is_some());
}
