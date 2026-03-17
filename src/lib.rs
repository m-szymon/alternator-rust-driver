pub use aws_sdk_dynamodb::*;

mod interceptors;
mod create_table_ext;
pub use create_table_ext::{CreateTableBuilderExt, CreateTableWithVectorIndexes};

#[cfg(test)]
mod tests {

    use super::*;
    use config::*;

    #[tokio::test]
    async fn example_unit_test() {
        let lhs = Config::builder().region(Region::new("us-west-2")).build();
        let rhs = Config::builder().region(Region::new("us-west-2")).build();

        assert_eq!(lhs.region(), rhs.region());
    }
}
