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

//! Post-decompression, pre-deserialization `FLOAT32VECTOR` response
//! transformation.
//!
//! Wraps an `SdkBody` with a lazy transformer that buffers the full
//! response and, for successful JSON bodies containing `FLOAT32VECTOR`
//! attribute values, rewrites them into a shape the generated SDK
//! deserializer understands (`L` of `N` by default, or a marker `B` when
//! `preserve_float32_vectors` is enabled) before handing the bytes on.
//!
//! Errors from the inner body are propagated unchanged. Empty bodies,
//! non-JSON bodies, bodies without `FLOAT32VECTOR`, and JSON parse failures
//! are passed through unchanged rather than turned into a local error: only
//! a successful, safe transformation replaces the original bytes.

use crate::float32_vector::rewrite_response_json_markers;

use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use futures_util::stream::Stream;
use http_body::Frame;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Wraps `body` with a lazy transformer. See module docs for behavior.
pub(crate) fn wrap_vector_response_body(body: SdkBody, preserve: bool) -> SdkBody {
    let stream = http_body_util::BodyStream::new(body);

    let body_impl = VectorTransformBody {
        inner: Box::pin(stream),
        buffer: Vec::new(),
        trailers: Vec::new(),
        preserve,
        done: false,
        emitted_data: false,
    };

    SdkBody::from_body_1_x(body_impl)
}

type BoxedError = Box<dyn std::error::Error + Send + Sync>;

struct VectorTransformBody {
    inner: Pin<Box<dyn Stream<Item = Result<Frame<Bytes>, BoxedError>> + Send + Sync>>,
    buffer: Vec<u8>,
    /// Non-data frames (i.e. trailers) observed on the inner stream, to be
    /// re-emitted unchanged after the transformed data frame.
    trailers: Vec<Frame<Bytes>>,
    preserve: bool,
    done: bool,
    emitted_data: bool,
}

impl http_body::Body for VectorTransformBody {
    type Data = Bytes;
    type Error = BoxedError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        loop {
            if !this.emitted_data {
                if !this.done {
                    let inner = this.inner.as_mut();
                    match inner.poll_next(cx) {
                        Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                            Ok(bytes) => {
                                this.buffer.extend_from_slice(&bytes);
                                continue;
                            }
                            Err(frame) => {
                                // A non-data frame (trailers): stash it to
                                // be emitted after the transformed data
                                // frame, since a single data frame must be
                                // emitted whole after full buffering.
                                this.trailers.push(frame);
                                continue;
                            }
                        },
                        Poll::Ready(Some(Err(e))) => {
                            // Propagate upstream errors as-is; do not
                            // attempt a transform on a partial/failed body.
                            this.emitted_data = true;
                            return Poll::Ready(Some(Err(e)));
                        }
                        Poll::Ready(None) => {
                            this.done = true;
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }

                this.emitted_data = true;
                let transformed = transform_bytes(&this.buffer, this.preserve);
                return Poll::Ready(Some(Ok(Frame::data(Bytes::from(transformed)))));
            }

            if let Some(frame) = this.trailers.pop() {
                return Poll::Ready(Some(Ok(frame)));
            }
            return Poll::Ready(None);
        }
    }
}

/// Transforms a fully-buffered response body, rewriting `FLOAT32VECTOR`
/// attributes. Returns the original bytes unchanged for empty, non-JSON,
/// `FLOAT32VECTOR`-free, or unparsable bodies.
fn transform_bytes(bytes: &[u8], preserve: bool) -> Vec<u8> {
    if bytes.is_empty() || !contains_subslice(bytes, b"FLOAT32VECTOR") {
        return bytes.to_vec();
    }

    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return bytes.to_vec();
    };

    rewrite_response_json_markers(&mut json, preserve);

    serde_json::to_vec(&json).unwrap_or_else(|_| bytes.to_vec())
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return needle.is_empty();
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::float32_vector::Float32VectorExt as _;
    use aws_sdk_dynamodb::primitives::Blob;
    use aws_sdk_dynamodb::types::AttributeValue;
    use base64::Engine as _;
    use futures_util::stream;
    use http_body::Body as _;
    use http_body_util::BodyExt;
    use http_body_util::StreamBody;

    async fn drain(body: SdkBody) -> Vec<u8> {
        let collected = body.collect().await.expect("body should not error");
        collected.to_bytes().to_vec()
    }

    /// Builds an [SdkBody] from multiple raw chunks, each delivered as its
    /// own `http_body::Frame::data`, to exercise the transformer's
    /// cross-frame buffering.
    fn multi_frame_body(chunks: Vec<&'static [u8]>) -> SdkBody {
        let frames: Vec<Result<Frame<Bytes>, BoxedError>> = chunks
            .into_iter()
            .map(|c| Ok(Frame::data(Bytes::from_static(c))))
            .collect();
        SdkBody::from_body_1_x(StreamBody::new(stream::iter(frames)))
    }

    #[tokio::test]
    async fn transforms_correctly_across_multiple_input_frames() {
        let original = serde_json::json!({
            "Item": { "embedding": { "FLOAT32VECTOR": [1.0, 2.0] } }
        });
        let bytes = serde_json::to_vec(&original).unwrap();
        // Split the JSON into two chunks delivered as separate frames.
        let mid = bytes.len() / 2;
        let first: &'static [u8] = Box::leak(bytes[..mid].to_vec().into_boxed_slice());
        let second: &'static [u8] = Box::leak(bytes[mid..].to_vec().into_boxed_slice());

        let body = wrap_vector_response_body(multi_frame_body(vec![first, second]), false);
        let out = drain(body).await;
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["Item"]["embedding"]["L"],
            serde_json::json!([{ "N": "1" }, { "N": "2" }])
        );
    }

    #[tokio::test]
    async fn preserves_trailers_across_transformation() {
        let original = serde_json::json!({
            "Item": { "embedding": { "FLOAT32VECTOR": [1.0, 2.0] } }
        });
        let bytes = serde_json::to_vec(&original).unwrap();

        let mut trailer_map = http::HeaderMap::new();
        trailer_map.insert(
            "x-amz-trailer-test",
            http::HeaderValue::from_static("present"),
        );

        let frames: Vec<Result<Frame<Bytes>, BoxedError>> = vec![
            Ok(Frame::data(Bytes::from(bytes))),
            Ok(Frame::trailers(trailer_map)),
        ];
        let input = SdkBody::from_body_1_x(StreamBody::new(stream::iter(frames)));

        let mut body = wrap_vector_response_body(input, false);
        let mut saw_trailers = false;
        loop {
            match std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
                Some(Ok(frame)) => {
                    if frame.is_trailers() {
                        saw_trailers = true;
                    }
                }
                Some(Err(e)) => panic!("body should not error: {e}"),
                None => break,
            }
        }

        assert!(
            saw_trailers,
            "trailers must survive vector response transformation"
        );
    }

    #[tokio::test]
    async fn passes_through_empty_body_unchanged() {
        let body = wrap_vector_response_body(SdkBody::empty(), false);
        assert_eq!(drain(body).await, Vec::<u8>::new());
    }

    #[tokio::test]
    async fn passes_through_non_json_body_unchanged() {
        let body = wrap_vector_response_body(SdkBody::from("not json FLOAT32VECTOR"), false);
        assert_eq!(drain(body).await, b"not json FLOAT32VECTOR".to_vec());
    }

    #[tokio::test]
    async fn passes_through_body_without_marker_unchanged() {
        let original = serde_json::json!({ "Item": { "pk": { "S": "x" } } });
        let bytes = serde_json::to_vec(&original).unwrap();
        let body = wrap_vector_response_body(SdkBody::from(bytes.clone()), false);
        assert_eq!(drain(body).await, bytes);
    }

    #[tokio::test]
    async fn converts_float32vector_to_l_n_by_default() {
        let original = serde_json::json!({
            "Item": { "embedding": { "FLOAT32VECTOR": [1.0, 2.0] } }
        });
        let bytes = serde_json::to_vec(&original).unwrap();
        let body = wrap_vector_response_body(SdkBody::from(bytes), false);
        let out = drain(body).await;
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["Item"]["embedding"]["L"],
            serde_json::json!([{ "N": "1" }, { "N": "2" }])
        );
    }

    #[tokio::test]
    async fn converts_float32vector_to_marker_binary_when_preserving() {
        let original = serde_json::json!({
            "Item": { "embedding": { "FLOAT32VECTOR": [1.0, 2.0] } }
        });
        let bytes = serde_json::to_vec(&original).unwrap();
        let body = wrap_vector_response_body(SdkBody::from(bytes), true);
        let out = drain(body).await;
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let b64 = parsed["Item"]["embedding"]["B"]
            .as_str()
            .expect("preserved vector is a B value");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(
            AttributeValue::B(Blob::new(bytes))
                .float32_vector()
                .unwrap(),
            vec![1.0, 2.0]
        );
    }

    #[tokio::test]
    async fn converts_float32vector_inside_search_results() {
        let original = serde_json::json!({
            "SearchResults": [
                { "Item": { "pk": { "S": "a" }, "embedding": { "FLOAT32VECTOR": [1.0, 0.0] } }, "Score": 0.0 },
                { "Item": { "pk": { "S": "b" } }, "Score": 0.5 }
            ]
        });
        let bytes = serde_json::to_vec(&original).unwrap();
        let body = wrap_vector_response_body(SdkBody::from(bytes), false);
        let out = drain(body).await;
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["SearchResults"][0]["Item"]["embedding"]["L"],
            serde_json::json!([{ "N": "1" }, { "N": "0" }])
        );
        assert_eq!(parsed["SearchResults"][0]["Score"], 0.0);
        assert_eq!(parsed["SearchResults"][1]["Item"]["pk"]["S"], "b");
    }
}
