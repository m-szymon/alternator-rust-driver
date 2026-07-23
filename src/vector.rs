//! Alternator vector-search extensions: typed request/response models and
//! extension traits for `VectorIndexes`, `VectorIndexUpdates`,
//! `VectorSearch`, and `Scores`, none of which the generated AWS SDK types
//! can express.
//!
//! Use [`crate::CreateTableVectorExt`], [`crate::QueryVectorExt`],
//! [`crate::UpdateTableVectorExt`], and [`crate::DescribeTableVectorExt`]
//! directly on the ordinary AWS SDK fluent builders returned by
//! [`crate::AlternatorClient::create_table`],
//! [`crate::AlternatorClient::query`],
//! [`crate::AlternatorClient::update_table`], and
//! [`crate::AlternatorClient::describe_table`]. Ordinary AWS SDK request
//! setters must precede the vector extension method; it is the boundary
//! after which only vector-specific and
//! [`crate::AlternatorCustomizableOperation::alternator_config_override`]
//! configuration remain available.
//!
//! See the crate README's "Vector search" section for end-to-end examples.
//! `FLOAT32VECTOR` attribute encoding lives in
//! [`crate::float32_vector`].

use aws_sdk_dynamodb::types::AttributeValue;
use serde::Serialize;

/// Similarity functions for vector search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityFunction {
    Cosine,
    Euclidean,
    DotProduct,
}

impl SimilarityFunction {
    pub fn as_str(&self) -> &'static str {
        match self {
            SimilarityFunction::Cosine => "COSINE",
            SimilarityFunction::Euclidean => "EUCLIDEAN",
            SimilarityFunction::DotProduct => "DOT_PRODUCT",
        }
    }
}

/// Defines a vector attribute in a vector index.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorAttribute {
    pub attribute_name: String,
    pub dimensions: u32,
}

impl VectorAttribute {
    pub fn builder() -> VectorAttributeBuilder {
        VectorAttributeBuilder::default()
    }
}

/// Builder for [VectorAttribute].
#[derive(Debug, Default)]
pub struct VectorAttributeBuilder {
    attribute_name: Option<String>,
    dimensions: Option<u32>,
}

impl VectorAttributeBuilder {
    pub fn attribute_name(mut self, name: impl Into<String>) -> Self {
        self.attribute_name = Some(name.into());
        self
    }

    pub fn dimensions(mut self, dimensions: u32) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    pub fn build(self) -> Result<VectorAttribute, &'static str> {
        let attribute_name = self.attribute_name.ok_or("attribute_name is required")?;
        if attribute_name.is_empty() {
            return Err("attribute_name must not be empty");
        }
        let dimensions = self.dimensions.ok_or("dimensions is required")?;
        if dimensions == 0 {
            return Err("dimensions must be positive");
        }
        if dimensions > 16000 {
            return Err("dimensions must not exceed 16000");
        }
        Ok(VectorAttribute {
            attribute_name,
            dimensions,
        })
    }
}

/// Projection type for a vector index.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionType {
    All,
    KeysOnly,
    Include(Vec<String>),
}

/// Projection configuration for a vector index.
///
/// A [VectorIndex] with no projection configured omits the `Projection`
/// field from the request entirely, so the server applies its own default.
/// This is distinct from explicitly requesting [ProjectionType::KeysOnly].
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub projection_type: ProjectionType,
}

impl Projection {
    pub fn all() -> Self {
        Self {
            projection_type: ProjectionType::All,
        }
    }

    pub fn keys_only() -> Self {
        Self {
            projection_type: ProjectionType::KeysOnly,
        }
    }

    pub fn include(attributes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            projection_type: ProjectionType::Include(
                attributes.into_iter().map(Into::into).collect(),
            ),
        }
    }
}

/// Status of a vector index, as reported in responses. Absent from
/// request JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
    Creating,
    Updating,
    Deleting,
    Active,
}

#[allow(dead_code)]
impl IndexStatus {
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "CREATING" => Some(IndexStatus::Creating),
            "UPDATING" => Some(IndexStatus::Updating),
            "DELETING" => Some(IndexStatus::Deleting),
            "ACTIVE" => Some(IndexStatus::Active),
            _ => None,
        }
    }
}

/// A vector index configuration for Alternator Vector Search.
///
/// The index automatically uses the base table's primary key schema
/// (HASH and optional RANGE). No `KeySchema` is specified here.
#[derive(Debug, Clone)]
pub struct VectorIndex {
    pub index_name: String,
    pub vector_attribute: VectorAttribute,
    /// When `None`, the `Projection` field is omitted from the request
    /// entirely and the server applies its own default projection.
    pub projection: Option<Projection>,
    pub similarity_function: Option<SimilarityFunction>,
    /// Response-only: the index's current status. Absent (`None`) for
    /// locally-built request values; populated when parsed from a
    /// `CreateTable`/`DescribeTable`/`UpdateTable` response.
    pub index_status: Option<IndexStatus>,
    /// Response-only: whether the index is still backfilling. Absent
    /// (`None`) for locally-built request values.
    pub backfilling: Option<bool>,
}

impl VectorIndex {
    pub fn builder() -> VectorIndexBuilder {
        VectorIndexBuilder::default()
    }
}

/// Builder for [VectorIndex].
#[derive(Debug, Default)]
pub struct VectorIndexBuilder {
    index_name: Option<String>,
    vector_attribute: Option<VectorAttribute>,
    projection: Option<Projection>,
    similarity_function: Option<SimilarityFunction>,
}

impl VectorIndexBuilder {
    pub fn index_name(mut self, name: impl Into<String>) -> Self {
        self.index_name = Some(name.into());
        self
    }

    pub fn vector_attribute(mut self, attr: VectorAttribute) -> Self {
        self.vector_attribute = Some(attr);
        self
    }

    pub fn projection(mut self, projection: Projection) -> Self {
        self.projection = Some(projection);
        self
    }

    pub fn similarity_function(mut self, func: SimilarityFunction) -> Self {
        self.similarity_function = Some(func);
        self
    }

    pub fn build(self) -> Result<VectorIndex, &'static str> {
        let index_name = self.index_name.ok_or("index_name is required")?;
        validate_index_name(&index_name)?;
        let vector_attribute = self
            .vector_attribute
            .ok_or("vector_attribute is required")?;
        if vector_attribute.dimensions == 0 {
            return Err("dimensions must be positive");
        }
        Ok(VectorIndex {
            index_name,
            vector_attribute,
            projection: self.projection,
            similarity_function: self.similarity_function,
            index_status: None,
            backfilling: None,
        })
    }
}

/// Validates an index name against the DynamoDB-style rule: 3-192 ASCII
/// characters matching `[a-zA-Z0-9._-]+`.
pub(crate) fn validate_index_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("index_name must not be empty");
    }
    if !(3..=192).contains(&name.len()) {
        return Err("index_name must be between 3 and 192 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err("index_name must match [a-zA-Z0-9._-]+");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VectorIndexJson {
    pub index_name: String,
    pub vector_attribute: VectorAttributeJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_function: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ProjectionJson {
    pub projection_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub non_key_attributes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VectorAttributeJson {
    pub attribute_name: String,
    pub dimensions: u32,
}

impl From<&VectorIndex> for VectorIndexJson {
    fn from(idx: &VectorIndex) -> Self {
        let projection = idx.projection.as_ref().map(|p| {
            let (projection_type, non_key_attributes) = match &p.projection_type {
                ProjectionType::All => ("ALL", None),
                ProjectionType::KeysOnly => ("KEYS_ONLY", None),
                ProjectionType::Include(attrs) => ("INCLUDE", Some(attrs.clone())),
            };
            ProjectionJson {
                projection_type: projection_type.to_string(),
                non_key_attributes,
            }
        });

        VectorIndexJson {
            index_name: idx.index_name.clone(),
            projection,
            vector_attribute: VectorAttributeJson {
                attribute_name: idx.vector_attribute.attribute_name.clone(),
                dimensions: idx.vector_attribute.dimensions,
            },
            similarity_function: idx.similarity_function.map(|f| f.as_str().to_string()),
        }
    }
}

/// Parses a `VectorIndex` response entry (as found under `Table.VectorIndexes`
/// or `TableDescription.VectorIndexes`) from raw JSON, including
/// response-only `IndexStatus` and `Backfilling` fields.
pub(crate) fn vector_index_from_json(value: &serde_json::Value) -> Option<VectorIndex> {
    let index_name = value.get("IndexName")?.as_str()?.to_string();
    let va = value.get("VectorAttribute")?;
    let attribute_name = va.get("AttributeName")?.as_str()?.to_string();
    let dimensions_raw = va.get("Dimensions")?.as_u64()?;
    let dimensions = u32::try_from(dimensions_raw).ok()?;

    let projection = value.get("Projection").and_then(|p| {
        let projection_type = p.get("ProjectionType")?.as_str()?;
        Some(match projection_type {
            "ALL" => Projection::all(),
            "KEYS_ONLY" => Projection::keys_only(),
            "INCLUDE" => {
                let attrs = p
                    .get("NonKeyAttributes")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Projection::include(attrs)
            }
            _ => return None,
        })
    });

    let similarity_function = value
        .get("SimilarityFunction")
        .and_then(|s| s.as_str())
        .and_then(|s| match s {
            "COSINE" => Some(SimilarityFunction::Cosine),
            "EUCLIDEAN" => Some(SimilarityFunction::Euclidean),
            "DOT_PRODUCT" => Some(SimilarityFunction::DotProduct),
            _ => None,
        });

    let index_status = value
        .get("IndexStatus")
        .and_then(|s| s.as_str())
        .and_then(IndexStatus::from_str);

    let backfilling = value.get("Backfilling").and_then(|b| b.as_bool());

    Some(VectorIndex {
        index_name,
        vector_attribute: VectorAttribute {
            attribute_name,
            dimensions,
        },
        projection,
        similarity_function,
        index_status,
        backfilling,
    })
}

/// A single vector-index change for `UpdateTable.VectorIndexUpdates`.
#[derive(Debug, Clone)]
pub enum VectorIndexUpdate {
    Create(VectorIndex),
    Delete { index_name: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VectorIndexUpdateJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<VectorIndexJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<VectorIndexDeleteJson>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VectorIndexDeleteJson {
    pub index_name: String,
}

impl From<&VectorIndexUpdate> for VectorIndexUpdateJson {
    fn from(update: &VectorIndexUpdate) -> Self {
        match update {
            VectorIndexUpdate::Create(idx) => VectorIndexUpdateJson {
                create: Some(idx.into()),
                delete: None,
            },
            VectorIndexUpdate::Delete { index_name } => VectorIndexUpdateJson {
                create: None,
                delete: Some(VectorIndexDeleteJson {
                    index_name: index_name.clone(),
                }),
            },
        }
    }
}

/// Score reporting mode for a [VectorSearch] query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnScores {
    /// Request similarity scores alongside query results.
    Similarity,
}

impl ReturnScores {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReturnScores::Similarity => "SIMILARITY",
        }
    }
}

/// Vector-search extras for `Query.VectorSearch`.
///
/// The query vector is stored as a validated DynamoDB [`AttributeValue`]:
/// either a compact `FLOAT32VECTOR` marker built by [`VectorSearch::new`],
/// or a standard `AttributeValue::L` of numeric (`N`) values built by
/// [`VectorSearch::from_query_vector`].
#[derive(Debug, Clone, PartialEq)]
pub struct VectorSearch {
    pub(crate) query_vector: AttributeValue,
    pub return_scores: Option<ReturnScores>,
}

impl VectorSearch {
    /// Creates a [VectorSearch] with a compact `FLOAT32VECTOR` query vector
    /// and no score reporting. Returns an error if `vector` is empty or
    /// contains non-finite values.
    pub fn new(vector: impl IntoIterator<Item = f32>) -> Result<Self, &'static str> {
        let vector: Vec<f32> = vector.into_iter().collect();
        if vector.is_empty() {
            return Err("query vector must not be empty");
        }
        if vector.iter().any(|v| !v.is_finite()) {
            return Err("query vector must contain only finite values");
        }
        let query_vector = crate::float32_vector::Float32Vector::to_attribute_value(vector)
            .expect("finiteness already validated above");
        Ok(Self {
            query_vector,
            return_scores: None,
        })
    }

    /// Creates a [VectorSearch] with a standard DynamoDB `AttributeValue::L`
    /// query vector, i.e. a list of `AttributeValue::N` values, matching the
    /// shape a non-preserving read returns. Returns an error if `values` is
    /// empty or contains a non-numeric member.
    pub fn from_query_vector(
        values: impl IntoIterator<Item = AttributeValue>,
    ) -> Result<Self, &'static str> {
        let values: Vec<AttributeValue> = values.into_iter().collect();
        if values.is_empty() {
            return Err("query vector must not be empty");
        }
        if !values.iter().all(|v| matches!(v, AttributeValue::N(_))) {
            return Err("query vector list must contain only numeric (N) values");
        }
        Ok(Self {
            query_vector: AttributeValue::L(values),
            return_scores: None,
        })
    }

    pub fn with_return_scores(mut self, return_scores: ReturnScores) -> Self {
        self.return_scores = Some(return_scores);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct VectorSearchJson {
    pub query_vector: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_scores: Option<String>,
}

impl From<&VectorSearch> for VectorSearchJson {
    fn from(search: &VectorSearch) -> Self {
        VectorSearchJson {
            query_vector: crate::float32_vector::attribute_value_to_json(&search.query_vector),
            return_scores: search.return_scores.map(|s| s.as_str().to_string()),
        }
    }
}

// --- Vector response-aware output wrappers ---
//
// These wrap the generated AWS SDK output types with Alternator-only
// response data. They are constructed by the response-aware operation
// wrappers in `crate::vector_interceptor` after `.send()` on the
// underlying `CustomizableOperation` completes.

use aws_sdk_dynamodb::operation::create_table::CreateTableOutput;
use aws_sdk_dynamodb::operation::describe_table::DescribeTableOutput;
use aws_sdk_dynamodb::operation::query::QueryOutput;

/// Wraps a generated [QueryOutput], adding similarity scores extracted from
/// the response's `Scores` field when [`VectorSearch::with_return_scores`]
/// requested them.
#[derive(Debug, Clone)]
pub struct VectorQueryOutput {
    output: QueryOutput,
    /// Present when the query requested [ReturnScores::Similarity]; absent
    /// (not a stale empty `Vec`) otherwise.
    pub scores: Option<Vec<f64>>,
}

impl VectorQueryOutput {
    pub(crate) fn new(output: QueryOutput, scores: Option<Vec<f64>>) -> Self {
        Self { output, scores }
    }

    /// Consumes this value, returning the generated [QueryOutput]. Rust does
    /// not perform this conversion implicitly.
    pub fn into_inner(self) -> QueryOutput {
        self.output
    }

    /// Borrows the generated [QueryOutput].
    pub fn as_inner(&self) -> &QueryOutput {
        &self.output
    }
}

impl From<VectorQueryOutput> for QueryOutput {
    fn from(value: VectorQueryOutput) -> Self {
        value.output
    }
}

/// Wraps a generated [DescribeTableOutput], adding parsed
/// `Table.VectorIndexes` metadata.
#[derive(Debug, Clone)]
pub struct DescribeTableWithVectorIndexes {
    output: DescribeTableOutput,
    pub vector_indexes: Vec<VectorIndex>,
}

impl DescribeTableWithVectorIndexes {
    pub(crate) fn new(output: DescribeTableOutput, vector_indexes: Vec<VectorIndex>) -> Self {
        Self {
            output,
            vector_indexes,
        }
    }

    /// Consumes this value, returning the generated [DescribeTableOutput].
    /// Rust does not perform this conversion implicitly.
    pub fn into_inner(self) -> DescribeTableOutput {
        self.output
    }

    /// Borrows the generated [DescribeTableOutput].
    pub fn as_inner(&self) -> &DescribeTableOutput {
        &self.output
    }
}

impl From<DescribeTableWithVectorIndexes> for DescribeTableOutput {
    fn from(value: DescribeTableWithVectorIndexes) -> Self {
        value.output
    }
}

/// Wraps a generated [CreateTableOutput], adding parsed
/// `TableDescription.VectorIndexes` metadata.
#[derive(Debug, Clone)]
pub struct CreateTableWithVectorIndexes {
    output: CreateTableOutput,
    pub vector_indexes: Vec<VectorIndex>,
}

impl CreateTableWithVectorIndexes {
    pub(crate) fn new(output: CreateTableOutput, vector_indexes: Vec<VectorIndex>) -> Self {
        Self {
            output,
            vector_indexes,
        }
    }

    /// Consumes this value, returning the generated [CreateTableOutput].
    /// Rust does not perform this conversion implicitly.
    pub fn into_inner(self) -> CreateTableOutput {
        self.output
    }

    /// Borrows the generated [CreateTableOutput].
    pub fn as_inner(&self) -> &CreateTableOutput {
        &self.output
    }
}

impl From<CreateTableWithVectorIndexes> for CreateTableOutput {
    fn from(value: CreateTableWithVectorIndexes) -> Self {
        value.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_function_as_str() {
        assert_eq!(SimilarityFunction::Cosine.as_str(), "COSINE");
        assert_eq!(SimilarityFunction::Euclidean.as_str(), "EUCLIDEAN");
        assert_eq!(SimilarityFunction::DotProduct.as_str(), "DOT_PRODUCT");
    }

    #[test]
    fn test_vector_attribute_builder() {
        let attr = VectorAttribute::builder()
            .attribute_name("embedding")
            .dimensions(128)
            .build()
            .unwrap();
        assert_eq!(attr.attribute_name, "embedding");
        assert_eq!(attr.dimensions, 128);
    }

    #[test]
    fn test_vector_index_builder() {
        let va = VectorAttribute::builder()
            .attribute_name("v")
            .dimensions(64)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("my_vec_idx")
            .vector_attribute(va)
            .similarity_function(SimilarityFunction::Cosine)
            .build()
            .unwrap();
        assert_eq!(idx.index_name, "my_vec_idx");
        assert_eq!(idx.vector_attribute.dimensions, 64);
        assert_eq!(idx.similarity_function, Some(SimilarityFunction::Cosine));
    }

    #[test]
    fn test_vector_index_json_default_projection() {
        let va = VectorAttribute::builder()
            .attribute_name("embedding")
            .dimensions(128)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .build()
            .unwrap();
        let json: VectorIndexJson = (&idx).into();
        let serialized = serde_json::to_value(&json).unwrap();

        assert_eq!(serialized["IndexName"], "vec_idx");
        assert_eq!(serialized["VectorAttribute"]["AttributeName"], "embedding");
        assert_eq!(serialized["VectorAttribute"]["Dimensions"], 128);
        // No projection was configured, so the field is omitted entirely
        // and the server applies its own default.
        assert!(serialized.get("Projection").is_none());
        // No similarity function should be absent
        assert!(serialized.get("SimilarityFunction").is_none());
    }

    #[test]
    fn test_vector_index_json_with_explicit_keys_only_projection() {
        let va = VectorAttribute::builder()
            .attribute_name("v")
            .dimensions(64)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .projection(Projection::keys_only())
            .build()
            .unwrap();
        let json: VectorIndexJson = (&idx).into();
        let serialized = serde_json::to_value(&json).unwrap();

        // Explicitly requested KEYS_ONLY is sent on the wire, unlike omission.
        assert_eq!(serialized["Projection"]["ProjectionType"], "KEYS_ONLY");
    }

    #[test]
    fn test_vector_index_json_with_cosine_similarity() {
        let va = VectorAttribute::builder()
            .attribute_name("v")
            .dimensions(64)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .similarity_function(SimilarityFunction::Cosine)
            .build()
            .unwrap();
        let json: VectorIndexJson = (&idx).into();
        let serialized = serde_json::to_value(&json).unwrap();

        assert_eq!(serialized["SimilarityFunction"], "COSINE");
    }

    #[test]
    fn test_vector_index_json_with_all_projection() {
        let va = VectorAttribute::builder()
            .attribute_name("v")
            .dimensions(64)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .projection(Projection::all())
            .build()
            .unwrap();
        let json: VectorIndexJson = (&idx).into();
        let serialized = serde_json::to_value(&json).unwrap();

        assert_eq!(serialized["Projection"]["ProjectionType"], "ALL");
    }

    #[test]
    fn test_index_name_validation() {
        assert!(validate_index_name("abc").is_ok());
        assert!(validate_index_name("ab").is_err(), "too short");
        assert!(validate_index_name(&"a".repeat(193)).is_err(), "too long");
        assert!(validate_index_name("valid-name.1_2").is_ok());
        assert!(
            validate_index_name("invalid name").is_err(),
            "space not allowed"
        );
        assert!(validate_index_name("invalid$name").is_err());
    }

    #[test]
    fn test_vector_attribute_builder_rejects_empty_name_and_zero_dimensions() {
        assert!(
            VectorAttribute::builder()
                .attribute_name("")
                .dimensions(1)
                .build()
                .is_err()
        );
        assert!(
            VectorAttribute::builder()
                .attribute_name("v")
                .dimensions(0)
                .build()
                .is_err()
        );
    }

    #[test]
    fn test_vector_attribute_builder_rejects_dimensions_over_16000() {
        assert!(
            VectorAttribute::builder()
                .attribute_name("embedding")
                .dimensions(16001)
                .build()
                .is_err(),
            "dimensions above 16000 should be rejected"
        );
        assert!(
            VectorAttribute::builder()
                .attribute_name("embedding")
                .dimensions(16000)
                .build()
                .is_ok(),
            "dimensions == 16000 should still be accepted"
        );
    }

    #[test]
    fn test_vector_index_update_create_json() {
        let va = VectorAttribute::builder()
            .attribute_name("v")
            .dimensions(64)
            .build()
            .unwrap();
        let idx = VectorIndex::builder()
            .index_name("vec_idx")
            .vector_attribute(va)
            .build()
            .unwrap();
        let update = VectorIndexUpdate::Create(idx);
        let json: VectorIndexUpdateJson = (&update).into();
        let serialized = serde_json::to_value(&json).unwrap();
        assert_eq!(serialized["Create"]["IndexName"], "vec_idx");
        assert!(serialized.get("Delete").is_none());
    }

    #[test]
    fn test_vector_index_update_delete_json() {
        let update = VectorIndexUpdate::Delete {
            index_name: "vec_idx".to_string(),
        };
        let json: VectorIndexUpdateJson = (&update).into();
        let serialized = serde_json::to_value(&json).unwrap();
        assert_eq!(serialized["Delete"]["IndexName"], "vec_idx");
        assert!(serialized.get("Create").is_none());
    }

    #[test]
    fn test_vector_search_json_without_scores() {
        let search = VectorSearch::new(vec![1.0, 2.0, 3.0]).unwrap();
        let json: VectorSearchJson = (&search).into();
        let serialized = serde_json::to_value(&json).unwrap();
        // The compact form serializes as a marker `AttributeValue::B`; the
        // driver's request interceptor rewrites it into
        // `{"FLOAT32VECTOR": [...]}` in the serialized wire body (see
        // `interceptors::tests::vector_search_is_injected_on_query`).
        assert!(serialized["QueryVector"].get("B").is_some());
        assert!(serialized.get("ReturnScores").is_none());
    }

    #[test]
    fn test_vector_search_json_with_scores() {
        let search = VectorSearch::new(vec![1.0])
            .unwrap()
            .with_return_scores(ReturnScores::Similarity);
        let json: VectorSearchJson = (&search).into();
        let serialized = serde_json::to_value(&json).unwrap();
        assert_eq!(serialized["ReturnScores"], "SIMILARITY");
    }

    #[test]
    fn test_vector_search_rejects_empty_vector() {
        assert!(VectorSearch::new(Vec::<f32>::new()).is_err());
    }

    #[test]
    fn test_vector_search_rejects_non_finite_values() {
        assert!(VectorSearch::new(vec![1.0, f32::NAN]).is_err());
        assert!(VectorSearch::new(vec![f32::INFINITY]).is_err());
    }

    #[test]
    fn test_vector_search_from_query_vector_json_uses_standard_list() {
        let search = VectorSearch::from_query_vector(vec![
            AttributeValue::N("1".to_string()),
            AttributeValue::N("2".to_string()),
            AttributeValue::N("3".to_string()),
        ])
        .unwrap();
        let json: VectorSearchJson = (&search).into();
        let serialized = serde_json::to_value(&json).unwrap();
        assert_eq!(
            serialized["QueryVector"]["L"],
            serde_json::json!([{ "N": "1" }, { "N": "2" }, { "N": "3" }])
        );
    }

    #[test]
    fn test_vector_search_from_query_vector_rejects_empty_and_non_numeric() {
        assert!(VectorSearch::from_query_vector(Vec::<AttributeValue>::new()).is_err());
        assert!(VectorSearch::from_query_vector(vec![AttributeValue::S("x".to_string())]).is_err());
    }

    #[test]
    fn test_vector_index_from_json_rejects_dimensions_over_u32_max() {
        // Phase 1 (VECTOR_5.md): Dimensions > u32::MAX must be rejected
        // rather than truncated by `as u32`. Currently
        // `vector_index_from_json` truncates via `as_u64()? as u32`, so
        // this fails: it returns `Some` with a truncated value instead of
        // `None`.
        let value = serde_json::json!({
            "IndexName": "vec_idx",
            "VectorAttribute": {
                "AttributeName": "embedding",
                "Dimensions": (u32::MAX as u64) + 1
            }
        });
        assert!(
            vector_index_from_json(&value).is_none(),
            "Dimensions overflowing u32 must be rejected, not truncated"
        );
    }

    #[test]
    fn test_vector_index_from_json_parses_status_and_backfilling() {
        let value = serde_json::json!({
            "IndexName": "vec_idx",
            "VectorAttribute": { "AttributeName": "embedding", "Dimensions": 128 },
            "Projection": { "ProjectionType": "KEYS_ONLY" },
            "SimilarityFunction": "COSINE",
            "IndexStatus": "ACTIVE",
            "Backfilling": false
        });
        let idx = vector_index_from_json(&value).unwrap();
        assert_eq!(idx.index_name, "vec_idx");
        assert_eq!(idx.vector_attribute.dimensions, 128);
        assert_eq!(idx.similarity_function, Some(SimilarityFunction::Cosine));
        assert_eq!(idx.index_status, Some(IndexStatus::Active));
        assert_eq!(idx.backfilling, Some(false));
    }
}
