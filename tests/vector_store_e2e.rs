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

//! Opt-in Vector Store integration coverage.
//!
//! Run with `RUSTFLAGS='--cfg ccm_tests' cargo test --test vector_store_e2e
//! -- --nocapture`. It requires Docker (for Vector Store only) plus these
//! variables:
//!
//! - `SCYLLA_VECTOR_STORE_IMAGE`: the Vector Store Docker image, e.g.
//!   `scylladb/vector-store:1.9.1`.
//! - `SCYLLA_VECTOR_STORE_PORT`: the Vector Store HTTP API port, e.g. `6080`.
//! - `SCYLLA_VECTOR_STORE_SCYLLA_VERSION`: the CCM-resolvable Scylla version
//!   used for the dedicated single-node cluster, e.g. `unstable/master:latest`.
//!   The DynamoDB-compatible Vector Search API (`SearchVectors`,
//!   `VectorIndexes` with `DistanceFunction`) requires a Scylla build that
//!   includes scylladb/scylladb#31181; released `release:*` versions do not
//!   have it yet. This is passed directly as `ccm create -v <value>`, so any
//!   value documented by `ccmlib.scylla_repository.setup` works (e.g.
//!   `unstable/master:<UTC timestamp>`).
//! - `SCYLLA_VECTOR_STORE_SCYLLA_CONFIG`: newline-separated `key:value`
//!   entries passed directly to `ccm node1 updateconf`. Must at minimum wire
//!   Scylla to the Vector Store instance, e.g.
//!   `vector_store_primary_uri:http://127.0.0.1:<SCYLLA_VECTOR_STORE_PORT>`.
//!
//! The Scylla cluster is created with CCM's normal package-based flow (`ccm
//! create -v ...`), not Docker, so it binds directly to a host loopback
//! alias (e.g. `127.0.2.1`) like every other CCM-based test in this repo. The
//! Vector Store container is started with `--network host` so it can reach
//! that address directly, and `VECTOR_STORE_URI` / `VECTOR_STORE_SCYLLADB_URI`
//! are set explicitly so both processes agree on `127.0.0.1` addressing.
//!
//! Example, using `make vector-store-e2e` (equivalent to the `cargo test`
//! invocation above):
//!
//! ```text
//! export RUSTFLAGS='--cfg ccm_tests'
//! export SCYLLA_VECTOR_STORE_IMAGE=scylladb/vector-store:1.9.1
//! export SCYLLA_VECTOR_STORE_PORT=6080
//! export SCYLLA_VECTOR_STORE_SCYLLA_VERSION='unstable/master:latest'
//! export SCYLLA_VECTOR_STORE_SCYLLA_CONFIG='vector_store_primary_uri:http://127.0.0.1:6080'
//! make vector-store-e2e
//! ```

mod ccm_wrapper;

use crate::ccm_wrapper::ccm::{Ccm, ClusterGuard};
use crate::ccm_wrapper::cluster::IpPrefix;
use crate::ccm_wrapper::topology_spec::{DatacenterSpec, TopologySpecBuilder};

use alternator_driver::*;
use anyhow::Context;
use aws_sdk_dynamodb::operation::search_vectors::SearchVectorsOutput;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, IndexStatus, KeySchemaElement, KeyType,
    Projection, ProjectionType, ScalarAttributeType, VectorAttributeDefinition,
    VectorDistanceFunction, VectorIndex, VectorIndexDescription,
};
use std::env;
use std::fmt::Write as _;
use std::process::Command;
use std::time::Duration;
use uuid::Uuid;

const INDEX_NAME: &str = "embedding_idx";
const VECTOR_ATTR: &str = "embedding";
const LABEL_ATTR: &str = "label";
const DIMENSIONS: i64 = 3;

struct VectorStoreContainer {
    name: String,
}

impl VectorStoreContainer {
    /// Starts the Vector Store container on the host network, pointed at
    /// the given Scylla (Alternator/CQL) address.
    fn start(image: &str, port: u16, scylladb_uri: &str) -> anyhow::Result<Self> {
        let name = format!("alternator-vector-store-{}", Uuid::new_v4());
        let status = Command::new("docker")
            .args(["run", "--detach", "--rm", "--network", "host", "--name"])
            .arg(&name)
            .arg("-e")
            .arg(format!("VECTOR_STORE_URI=127.0.0.1:{port}"))
            .arg("-e")
            .arg(format!("VECTOR_STORE_SCYLLADB_URI={scylladb_uri}"))
            .arg(image)
            .status()
            .context("failed to start Docker Vector Store container")?;
        anyhow::ensure!(
            status.success(),
            "Docker failed to start Vector Store container"
        );
        Ok(Self { name })
    }

    /// Returns `true` if the container is still running.
    fn is_running(&self) -> bool {
        let output = Command::new("docker")
            .args(["inspect", "--format", "{{.State.Running}}", &self.name])
            .output();
        matches!(output, Ok(o) if o.status.success() && o.stdout == b"true\n")
    }

    /// Returns the container's combined stdout/stderr logs.
    fn logs(&self) -> String {
        let output = Command::new("docker").args(["logs", &self.name]).output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                format!("stdout:\n{stdout}\nstderr:\n{stderr}")
            }
            Err(e) => format!("failed to get container logs: {e}"),
        }
    }
}

impl Drop for VectorStoreContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .status();
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    env::var(name).with_context(|| format!("{name} must be set for the Vector Store E2E test"))
}

struct VectorStoreConfig {
    image: String,
    port: u16,
    scylla_version: String,
    node_config: Vec<String>,
}

fn vector_store_config() -> anyhow::Result<VectorStoreConfig> {
    let image = required_env("SCYLLA_VECTOR_STORE_IMAGE")?;
    let port = required_env("SCYLLA_VECTOR_STORE_PORT")?
        .parse()
        .context("SCYLLA_VECTOR_STORE_PORT must be a valid port")?;
    let scylla_version = required_env("SCYLLA_VECTOR_STORE_SCYLLA_VERSION")?;
    let node_config: Vec<String> = required_env("SCYLLA_VECTOR_STORE_SCYLLA_CONFIG")?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    anyhow::ensure!(
        !node_config.is_empty() && node_config.iter().all(|entry| entry.contains(':')),
        "SCYLLA_VECTOR_STORE_SCYLLA_CONFIG must contain newline-separated key:value entries"
    );
    Ok(VectorStoreConfig {
        image,
        port,
        scylla_version,
        node_config,
    })
}

fn vector_client(endpoint_url: String, preserve_float32_vectors: bool) -> AlternatorClient {
    AlternatorClient::from_conf(
        AlternatorConfig::builder()
            .endpoint_url(endpoint_url)
            .seed_hosts(Vec::<String>::new())
            .behavior_version_latest()
            .allow_no_auth()
            .preserve_float32_vectors(preserve_float32_vectors)
            .build(),
    )
}

/// Formats vector-index descriptions for timeout diagnostics, so a failed
/// wait for `Active` shows what was last observed instead of nothing.
fn describe_index_metadata(indexes: &[VectorIndexDescription]) -> String {
    if indexes.is_empty() {
        return "<no vector indexes observed>".to_string();
    }
    let mut out = String::new();
    for index in indexes {
        let _ = write!(
            out,
            "\n  - index_name={:?} attribute={:?} dimensions={:?} status={:?} backfilling={:?}",
            index.index_name(),
            index.vector_attribute().map(|a| a.attribute_name()),
            index.dimensions(),
            index.index_status(),
            index.backfilling(),
        );
    }
    out
}

/// Polls `DescribeTable` for `table_name` until `INDEX_NAME` reports
/// [`IndexStatus::Active`], returning its final description. On timeout,
/// the error includes the last observed vector-index metadata.
async fn wait_for_index_active(
    client: &AlternatorClient,
    table_name: &str,
) -> anyhow::Result<VectorIndexDescription> {
    let mut last_observed: Vec<VectorIndexDescription> = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let described = client
                .describe_table()
                .table_name(table_name)
                .send()
                .await?;
            last_observed = described
                .table()
                .map(|table| table.vector_indexes().to_vec())
                .unwrap_or_default();
            if let Some(index) = last_observed.iter().find(|index| {
                index.index_name() == Some(INDEX_NAME)
                    && index.index_status() == Some(&IndexStatus::Active)
            }) {
                return Ok::<_, anyhow::Error>(index.clone());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => anyhow::bail!(
            "timed out waiting for vector index '{INDEX_NAME}' to become ACTIVE; last observed metadata:{}",
            describe_index_metadata(&last_observed)
        ),
    }
}

/// Asserts vector-index metadata echoed by the server matches what was
/// configured (name, attribute, dimensions, distance function), independent
/// of `IndexStatus`.
///
/// Only the index configuration is checked here, because `CreateTable`'s
/// response echoes back the request's own `VectorIndexes` entries rather
/// than a server-built description: server-derived fields such as
/// `IndexArn`, `IndexStatus` and `Backfilling` appear only in
/// `DescribeTable` (see [`assert_index_is_described`]).
fn assert_index_config_matches(index: &VectorIndexDescription) {
    assert_eq!(index.index_name(), Some(INDEX_NAME));
    assert_eq!(
        index.vector_attribute().map(|a| a.attribute_name()),
        Some(VECTOR_ATTR)
    );
    assert_eq!(index.dimensions(), Some(DIMENSIONS));
    assert_eq!(
        index.distance_function(),
        Some(&VectorDistanceFunction::Cosine)
    );
}

/// Asserts a `DescribeTable` vector-index description carries both the
/// configuration and the server-derived fields that only `DescribeTable`
/// reports.
fn assert_index_is_described(index: &VectorIndexDescription) {
    assert_index_config_matches(index);
    assert!(
        index
            .index_arn()
            .is_some_and(|arn| arn.contains(INDEX_NAME)),
        "DescribeTable should report an IndexArn naming the index, got {:?}",
        index.index_arn()
    );
}

fn result_keys(output: &SearchVectorsOutput) -> Vec<&str> {
    output
        .search_results()
        .iter()
        .map(|result| {
            result
                .item()
                .and_then(|item| item.get("pk"))
                .and_then(|v| v.as_s().ok())
                .map(String::as_str)
                .unwrap_or_default()
        })
        .collect()
}

#[tokio::test]
#[cfg_attr(not(ccm_tests), ignore)]
async fn vector_store_end_to_end() -> anyhow::Result<()> {
    let config = vector_store_config()?;

    let topology = TopologySpecBuilder::new()
        .datacenter(DatacenterSpec::new().rack(1))
        .build()?;
    let cluster_name = format!("vector-store-{}", Uuid::new_v4());
    let mut cluster = ClusterGuard(Ccm::create_cluster_with_node_config(
        cluster_name,
        &topology,
        IpPrefix::new("127.0.2.")?,
        8001,
        config.scylla_version,
        &config.node_config,
    )?);
    Ccm::start_cluster(&mut cluster)?;

    let node_ip = cluster.nodes()[0].ip.clone();
    let scylladb_uri = format!("{node_ip}:9042");

    // Vector Store must be started only after Scylla is reachable; the
    // container connects to ScyllaDB on first use.
    let _vector_store = VectorStoreContainer::start(&config.image, config.port, &scylladb_uri)?;

    // Wait for the Vector Store HTTP server to be reachable before proceeding.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if !_vector_store.is_running() {
                anyhow::bail!(
                    "Vector Store container exited unexpectedly:\n{}",
                    _vector_store.logs()
                );
            }
            if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", config.port))
                .await
                .is_ok()
            {
                break Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .context("Vector Store container did not become ready")??;

    let endpoint = cluster.nodes()[0].address();
    let client = vector_client(endpoint.clone(), false);
    let preserving_client = vector_client(endpoint, true);
    let table_name = format!("vector_store_{}", Uuid::new_v4().simple());

    // The index definition is the plain AWS SDK type: Alternator implements
    // DynamoDB's own Vector Search API.
    let index = VectorIndex::builder()
        .index_name(INDEX_NAME)
        .vector_attribute(
            VectorAttributeDefinition::builder()
                .attribute_name(VECTOR_ATTR)
                .build()?,
        )
        .dimensions(DIMENSIONS)
        .distance_function(VectorDistanceFunction::Cosine)
        .projection(
            Projection::builder()
                .projection_type(ProjectionType::KeysOnly)
                .build(),
        )
        .build()?;

    let result = client
        .create_table()
        .table_name(&table_name)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()?,
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()?,
        )
        .billing_mode(BillingMode::PayPerRequest)
        .vector_indexes(index)
        .send()
        .await?;
    let created_indexes = result
        .table_description()
        .context("CreateTable should return a TableDescription")?
        .vector_indexes();
    assert_eq!(created_indexes.len(), 1);
    assert_index_config_matches(&created_indexes[0]);

    let test_result: anyhow::Result<()> = async {
        let active_index = wait_for_index_active(&client, &table_name).await?;
        assert_index_is_described(&active_index);

        // Deterministic corpus: two vectors near the (1,0,0) axis, one near
        // the (0,1,0) axis, so a search for (1,0,0) has an unambiguous
        // nearest-first order. `label` is not projected into the index
        // (KEYS_ONLY), so it is only visible with Alternator's BaseRead.
        for (key, embedding, label) in [
            ("near_a", [1.0, 0.0, 0.0], "x"),
            ("near_b", [0.9, 0.1, 0.0], "y"),
            ("far", [0.0, 1.0, 0.0], "x"),
        ] {
            client
                .put_item()
                .table_name(&table_name)
                .item("pk", AttributeValue::S(key.into()))
                .item(
                    VECTOR_ATTR,
                    Float32Vector::to_attribute_value(embedding).unwrap(),
                )
                .item(LABEL_ATTR, AttributeValue::S(label.into()))
                .send()
                .await?;
        }

        // Plain DynamoDB-style SearchVectors with a list-of-numbers search
        // vector. The Vector Store indexes items asynchronously after they
        // are written to ScyllaDB, so poll until it has caught up.
        let search = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let output = client
                    .search_vectors()
                    .table_name(&table_name)
                    .index_name(INDEX_NAME)
                    .search_vector(AttributeValue::N("1".into()))
                    .search_vector(AttributeValue::N("0".into()))
                    .search_vector(AttributeValue::N("0".into()))
                    .top_k(2)
                    .send()
                    .await?;
                if output.search_results().len() == 2 {
                    return Ok::<_, anyhow::Error>(output);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .context("timed out waiting for Vector Store to index items")??;
        assert_search_result_is_nearest_first(&search);
        // KEYS_ONLY projection and BaseRead=false (the default): only keys.
        for result in search.search_results() {
            let item = result.item().unwrap();
            assert!(item.contains_key("pk"));
            assert!(!item.contains_key(LABEL_ATTR));
        }

        // The same search with a compact FLOAT32VECTOR search vector.
        let compact_search = client
            .search_vectors()
            .table_name(&table_name)
            .index_name(INDEX_NAME)
            .search_vector(Float32Vector::to_attribute_value([1.0, 0.0, 0.0])?)
            .top_k(2)
            .send()
            .await?;
        assert_search_result_is_nearest_first(&compact_search);

        // BaseRead=true reads the full item from the base table, exposing
        // the non-projected `label` attribute.
        let base_read = client
            .search_vectors()
            .table_name(&table_name)
            .index_name(INDEX_NAME)
            .search_vector(Float32Vector::to_attribute_value([1.0, 0.0, 0.0])?)
            .top_k(2)
            .base_read(true)
            .send()
            .await?;
        assert_eq!(result_keys(&base_read), vec!["near_a", "near_b"]);
        assert_eq!(
            base_read.search_results()[0].item().unwrap()[LABEL_ATTR],
            AttributeValue::S("x".into())
        );
        assert_eq!(
            base_read.search_results()[1].item().unwrap()[LABEL_ATTR],
            AttributeValue::S("y".into())
        );

        // FilterExpression post-filters the TopK candidates: of the two
        // nearest, only `near_a` has label "x".
        let filtered = client
            .search_vectors()
            .table_name(&table_name)
            .index_name(INDEX_NAME)
            .search_vector(Float32Vector::to_attribute_value([1.0, 0.0, 0.0])?)
            .top_k(2)
            .expression_attribute_names("#l", LABEL_ATTR)
            .expression_attribute_values(":l", AttributeValue::S("x".into()))
            .base_read(true)
            .filter_expression("#l = :l")
            .send()
            .await?;
        assert_eq!(result_keys(&filtered), vec!["near_a"]);

        // Default client reads the vector attribute as an ordinary L/N list.
        let item = client
            .get_item()
            .table_name(&table_name)
            .key("pk", AttributeValue::S("near_a".into()))
            .send()
            .await?
            .item()
            .cloned()
            .context("missing inserted item")?;
        assert!(matches!(item[VECTOR_ATTR], AttributeValue::L(_)));

        // Preserving client reads the same attribute as a compact vector.
        let preserved = preserving_client
            .get_item()
            .table_name(&table_name)
            .key("pk", AttributeValue::S("near_a".into()))
            .send()
            .await?
            .item()
            .cloned()
            .context("missing inserted item")?;
        assert_eq!(
            preserved[VECTOR_ATTR].float32_vector()?,
            vec![1.0, 0.0, 0.0]
        );

        // Writing the preserved item back unchanged, then reading it again
        // with preservation enabled, must still yield a compact vector
        // (round trip does not silently widen it to L/N).
        preserving_client
            .put_item()
            .table_name(&table_name)
            .set_item(Some(preserved))
            .send()
            .await?;
        let roundtripped = preserving_client
            .get_item()
            .table_name(&table_name)
            .key("pk", AttributeValue::S("near_a".into()))
            .send()
            .await?
            .item()
            .cloned()
            .context("missing item after preserving round trip")?;
        assert_eq!(
            roundtripped[VECTOR_ATTR].float32_vector()?,
            vec![1.0, 0.0, 0.0]
        );

        Ok(())
    }
    .await;

    let cleanup_result = client
        .delete_table()
        .table_name(&table_name)
        .send()
        .await
        .context("failed to delete Vector Store E2E table");
    match (test_result, cleanup_result) {
        (Err(test_error), Ok(_)) => {
            eprintln!("Vector Store container logs:\n{}", _vector_store.logs());
            Err(test_error)
        }
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(test_error), Err(cleanup_error)) => {
            eprintln!("Vector Store container logs:\n{}", _vector_store.logs());
            Err(test_error.context(format!("additionally, cleanup failed: {cleanup_error}")))
        }
        (Ok(()), Ok(_)) => Ok(()),
    }
}

/// Asserts a `SearchVectors` result over the `near_a`/`near_b`/`far` corpus
/// is nearest-first for a search vector at `(1,0,0)`: exactly the two
/// requested (`top_k(2)`) results, `near_a` before `near_b`, with finite
/// COSINE distance scores in ascending order (nearest first).
fn assert_search_result_is_nearest_first(output: &SearchVectorsOutput) {
    assert_eq!(
        output.search_results().len(),
        2,
        "expected exactly 2 results from top_k(2)"
    );
    assert_eq!(
        result_keys(output),
        vec!["near_a", "near_b"],
        "results must be ordered nearest-first"
    );

    let scores: Vec<f64> = output
        .search_results()
        .iter()
        .map(|result| result.score())
        .collect();
    assert!(
        scores.iter().all(|score| score.is_finite()),
        "all scores must be finite"
    );
    assert!(
        scores.windows(2).all(|pair| pair[0] <= pair[1]),
        "COSINE scores are distances and must be ascending (nearest first): {scores:?}"
    );
}
