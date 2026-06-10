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
}
