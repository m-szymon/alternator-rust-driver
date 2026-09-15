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

//! ScyllaDB-only extensions to DynamoDB's `SearchVectors` operation.
//!
//! Alternator implements the same Vector Search API as Amazon DynamoDB, so
//! vector indexes (`CreateTable.VectorIndexes`, `UpdateTable.VectorIndexUpdates`,
//! `DescribeTable`) and the `SearchVectors` operation are used through the
//! ordinary generated `aws-sdk-dynamodb` builders. On top of that, Alternator
//! accepts two request parameters on `SearchVectors` that DynamoDB does not
//! have, exposed here via [`SearchVectorsExt`]:
//!
//! - `BaseRead` (boolean, default `false`): whether to read matching items
//!   from the base table instead of serving the response purely from the
//!   attributes projected into the vector index.
//! - `FilterExpression`: a post-filter applied to the `TopK` candidates found
//!   by the approximate-nearest-neighbour search, with the same syntax as
//!   `Query`/`Scan`'s `FilterExpression`.
//!
//! The third extension, the compact `FLOAT32VECTOR` type (also accepted as
//! the `SearchVector` of a search), lives in [`crate::float32_vector`].

use aws_sdk_dynamodb::client::customize::CustomizableOperation;
use aws_sdk_dynamodb::operation::search_vectors::builders::SearchVectorsFluentBuilder;
use aws_sdk_dynamodb::operation::search_vectors::{SearchVectorsError, SearchVectorsOutput};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::interceptors::context::BeforeSerializationInterceptorContextMut;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::{ConfigBag, Storable, StoreReplace};

/// ScyllaDB-only `SearchVectors` request fields carried through [`ConfigBag`]
/// for [`crate::AlternatorInterceptor`] to inject into the serialized JSON
/// body immediately before compression.
///
/// Retry-safe: it never inspects or mutates the HTTP body itself, only
/// carries caller intent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SearchVectorsExtensions {
    pub(crate) base_read: Option<bool>,
    pub(crate) filter_expression: Option<String>,
}

impl SearchVectorsExtensions {
    fn merge_from(&mut self, other: &Self) {
        if other.base_read.is_some() {
            self.base_read = other.base_read;
        }
        if other.filter_expression.is_some() {
            self.filter_expression = other.filter_expression.clone();
        }
    }
}

impl Storable for SearchVectorsExtensions {
    type Storer = StoreReplace<Self>;
}

/// Per-operation interceptor that merges one or more
/// [`SearchVectorsExtensions`] fields into [`ConfigBag`] state.
#[derive(Debug, Clone)]
struct SearchVectorsExtensionsInterceptor {
    extensions: SearchVectorsExtensions,
}

impl Intercept for SearchVectorsExtensionsInterceptor {
    fn name(&self) -> &'static str {
        "SearchVectorsExtensionsInterceptor"
    }

    fn modify_before_serialization(
        &self,
        _: &mut BeforeSerializationInterceptorContextMut<'_>,
        _: &RuntimeComponents,
        cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let mut merged = cfg
            .interceptor_state()
            .load::<SearchVectorsExtensions>()
            .cloned()
            .unwrap_or_default();
        merged.merge_from(&self.extensions);
        cfg.interceptor_state().store_put(merged);
        Ok(())
    }
}

/// The generated `SearchVectors` customizable operation type.
pub type SearchVectorsOperation =
    CustomizableOperation<SearchVectorsOutput, SearchVectorsError, SearchVectorsFluentBuilder>;

/// Extension trait adding Alternator's `BaseRead` and `FilterExpression`
/// parameters to the generated `SearchVectors` builder.
///
/// Implemented for both [`SearchVectorsFluentBuilder`] and its
/// [`CustomizableOperation`], so misuse on other operations is a compile
/// error rather than a runtime one. Ordinary AWS SDK request setters
/// (`.table_name(...)`, `.top_k(...)`, `.search_vector(...)`, etc.) must
/// precede the first extension call: it is the boundary after which only
/// extension methods,
/// [`alternator_config_override`](crate::AlternatorCustomizableOperation::alternator_config_override),
/// and `.send()` remain available.
///
/// ```no_run
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use alternator_driver::{AlternatorClient, AlternatorConfig, Float32Vector, SearchVectorsExt};
/// use aws_sdk_dynamodb::types::AttributeValue;
///
/// let client = AlternatorClient::from_conf(
///     AlternatorConfig::builder().behavior_version_latest().build(),
/// );
///
/// let output = client
///     .search_vectors()
///     .table_name("Documents")
///     .index_name("embedding_idx")
///     .search_vector(Float32Vector::to_attribute_value([0.1, 0.2, 0.3]).unwrap())
///     .top_k(10)
///     .expression_attribute_values(":lang", AttributeValue::S("en".into()))
///     .base_read(true)                       // <-- extension boundary
///     .filter_expression("lang = :lang")
///     .send()
///     .await
///     .unwrap();
/// for result in output.search_results() {
///     println!("{:?} score={}", result.item(), result.score());
/// }
/// # });
/// ```
///
/// The extension is only defined for `SearchVectors`:
///
/// ```compile_fail
/// use alternator_driver::{AlternatorClient, AlternatorConfig, SearchVectorsExt};
///
/// let client = AlternatorClient::from_conf(
///     AlternatorConfig::builder().behavior_version_latest().build(),
/// );
/// let _ = client.query().table_name("t").base_read(true);
/// ```
pub trait SearchVectorsExt {
    /// The type returned by the extension methods.
    type Output;

    /// Sets Alternator's `BaseRead` parameter.
    ///
    /// With `false` (the server default, and the only mode Amazon DynamoDB
    /// supports) the response is built entirely from the attributes
    /// projected into the vector index. With `true`, each matching item is
    /// read from the base table instead, so `ProjectionExpression` and
    /// [`filter_expression`](Self::filter_expression) can reference any
    /// attribute of the item, at the cost of an extra read per result.
    fn base_read(self, base_read: bool) -> Self::Output;

    /// Sets Alternator's `FilterExpression` parameter: a post-filter applied
    /// to the `TopK` candidates found by the nearest-neighbour search. Uses
    /// the same syntax and `ExpressionAttributeNames`/`ExpressionAttributeValues`
    /// placeholders as `Query`'s `FilterExpression`. Because filtering
    /// happens after candidate selection, fewer than `TopK` results may be
    /// returned.
    fn filter_expression(self, expression: impl Into<String>) -> Self::Output;
}

impl SearchVectorsExt for SearchVectorsFluentBuilder {
    type Output = SearchVectorsOperation;

    fn base_read(self, base_read: bool) -> Self::Output {
        self.customize().base_read(base_read)
    }

    fn filter_expression(self, expression: impl Into<String>) -> Self::Output {
        self.customize().filter_expression(expression)
    }
}

impl SearchVectorsExt for SearchVectorsOperation {
    type Output = Self;

    fn base_read(self, base_read: bool) -> Self::Output {
        self.interceptor(SearchVectorsExtensionsInterceptor {
            extensions: SearchVectorsExtensions {
                base_read: Some(base_read),
                filter_expression: None,
            },
        })
    }

    fn filter_expression(self, expression: impl Into<String>) -> Self::Output {
        self.interceptor(SearchVectorsExtensionsInterceptor {
            extensions: SearchVectorsExtensions {
                base_read: None,
                filter_expression: Some(expression.into()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_overrides_only_set_fields() {
        let mut base = SearchVectorsExtensions {
            base_read: Some(false),
            filter_expression: Some("a = :a".into()),
        };
        base.merge_from(&SearchVectorsExtensions {
            base_read: Some(true),
            filter_expression: None,
        });
        assert_eq!(base.base_read, Some(true));
        assert_eq!(base.filter_expression.as_deref(), Some("a = :a"));

        base.merge_from(&SearchVectorsExtensions {
            base_read: None,
            filter_expression: Some("b = :b".into()),
        });
        assert_eq!(base.base_read, Some(true));
        assert_eq!(base.filter_expression.as_deref(), Some("b = :b"));
    }
}
