// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

#[cfg(test)]
mod tests {

    use crate::datasource::MemTable;
    use crate::datasource::{provider_as_source, DefaultTableSource};
    use crate::physical_plan::collect;
    use crate::prelude::SessionContext;
    use arrow::array::{AsArray, Int32Array};
    use arrow::datatypes::{DataType, Field, Schema, UInt64Type};
    use arrow::error::ArrowError;
    use arrow::record_batch::RecordBatch;
    use arrow_schema::SchemaRef;
    use datafusion_catalog::TableProvider;
    use datafusion_common::{DataFusionError, Result};
    use datafusion_expr::dml::InsertOp;
    use datafusion_expr::LogicalPlanBuilder;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_with_projection() -> Result<()> {
        let session_ctx = SessionContext::new();
        let task_ctx = session_ctx.task_ctx();
        let schema = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
            Field::new("d", DataType::Int32, true),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
                Arc::new(Int32Array::from(vec![None, None, Some(9)])),
            ],
        )?;

        let provider = MemTable::try_new(schema, vec![vec![batch]])?;

        // scan with projection
        let exec = provider
            .scan(&session_ctx.state(), Some(&vec![2, 1]), &[], None)
            .await?;

        let mut it = exec.execute(0, task_ctx)?;
        let batch2 = it.next().await.unwrap()?;
        assert_eq!(2, batch2.schema().fields().len());
        assert_eq!("c", batch2.schema().field(0).name());
        assert_eq!("b", batch2.schema().field(1).name());
        assert_eq!(2, batch2.num_columns());

        Ok(())
    }

    #[tokio::test]
    async fn test_without_projection() -> Result<()> {
        let session_ctx = SessionContext::new();
        let task_ctx = session_ctx.task_ctx();
        let schema = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )?;

        let provider = MemTable::try_new(schema, vec![vec![batch]])?;

        let exec = provider.scan(&session_ctx.state(), None, &[], None).await?;
        let mut it = exec.execute(0, task_ctx)?;
        let batch1 = it.next().await.unwrap()?;
        assert_eq!(3, batch1.schema().fields().len());
        assert_eq!(3, batch1.num_columns());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_projection() -> Result<()> {
        let session_ctx = SessionContext::new();

        let schema = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )?;

        let provider = MemTable::try_new(schema, vec![vec![batch]])?;

        let projection: Vec<usize> = vec![0, 4];

        match provider
            .scan(&session_ctx.state(), Some(&projection), &[], None)
            .await
        {
            Err(DataFusionError::ArrowError(err, _)) => match err.as_ref() {
                ArrowError::SchemaError(e) => {
                    assert_eq!(
                        "\"project index 4 out of bounds, max field 3\"",
                        format!("{e:?}")
                    )
                }
                _ => panic!("unexpected error"),
            },
            res => panic!("Scan should failed on invalid projection, got {res:?}"),
        };

        Ok(())
    }

    #[test]
    fn test_schema_validation_incompatible_column() -> Result<()> {
        let schema1 = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let schema2 = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Float64, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let batch = RecordBatch::try_new(
            schema1,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )?;

        let e = MemTable::try_new(schema2, vec![vec![batch]]).unwrap_err();
        assert_eq!(
            "Error during planning: Mismatch between schema and batches",
            e.strip_backtrace()
        );

        Ok(())
    }

    #[test]
    fn test_schema_validation_different_column_count() -> Result<()> {
        let schema1 = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let schema2 = Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]));

        let batch = RecordBatch::try_new(
            schema1,
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![7, 5, 9])),
            ],
        )?;

        let e = MemTable::try_new(schema2, vec![vec![batch]]).unwrap_err();
        assert_eq!(
            "Error during planning: Mismatch between schema and batches",
            e.strip_backtrace()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_merged_schema() -> Result<()> {
        let session_ctx = SessionContext::new();
        let task_ctx = session_ctx.task_ctx();
        let mut metadata = HashMap::new();
        metadata.insert("foo".to_string(), "bar".to_string());

        let schema1 = Schema::new_with_metadata(
            vec![
                Field::new("a", DataType::Int32, false),
                Field::new("b", DataType::Int32, false),
                Field::new("c", DataType::Int32, false),
            ],
            // test for comparing metadata
            metadata,
        );

        let schema2 = Schema::new(vec![
            // test for comparing nullability
            Field::new("a", DataType::Int32, true),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]);

        let merged_schema = Schema::try_merge(vec![schema1.clone(), schema2.clone()])?;

        let batch1 = RecordBatch::try_new(
            Arc::new(schema1),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )?;

        let batch2 = RecordBatch::try_new(
            Arc::new(schema2),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )?;

        let provider =
            MemTable::try_new(Arc::new(merged_schema), vec![vec![batch1, batch2]])?;

        let exec = provider.scan(&session_ctx.state(), None, &[], None).await?;
        let mut it = exec.execute(0, task_ctx)?;
        let batch1 = it.next().await.unwrap()?;
        assert_eq!(3, batch1.schema().fields().len());
        assert_eq!(3, batch1.num_columns());

        Ok(())
    }

    async fn experiment(
        schema: SchemaRef,
        initial_data: Vec<Vec<RecordBatch>>,
        inserted_data: Vec<Vec<RecordBatch>>,
    ) -> Result<Vec<Vec<RecordBatch>>> {
        let expected_count: u64 = inserted_data
            .iter()
            .flat_map(|batches| batches.iter().map(|batch| batch.num_rows() as u64))
            .sum();

        // Create a new session context
        let session_ctx = SessionContext::new();
        // Create and register the initial table with the provided schema and data
        let initial_table = Arc::new(MemTable::try_new(schema.clone(), initial_data)?);
        session_ctx.register_table("t", initial_table.clone())?;
        let target = Arc::new(DefaultTableSource::new(initial_table.clone()));
        // Create and register the source table with the provided schema and inserted data
        let source_table = Arc::new(MemTable::try_new(schema.clone(), inserted_data)?);
        session_ctx.register_table("source", source_table.clone())?;
        // Convert the source table into a provider so that it can be used in a query
        let source = provider_as_source(source_table);
        // Create a table scan logical plan to read from the source table
        let scan_plan = LogicalPlanBuilder::scan("source", source, None)?.build()?;
        // Create an insert plan to insert the source data into the initial table
        let insert_into_table =
            LogicalPlanBuilder::insert_into(scan_plan, "t", target, InsertOp::Append)?
                .build()?;
        // Create a physical plan from the insert plan
        let plan = session_ctx
            .state()
            .create_physical_plan(&insert_into_table)
            .await?;

        // Execute the physical plan and collect the results
        let res = collect(plan, session_ctx.task_ctx()).await?;
        assert_eq!(extract_count(res), expected_count);

        // Read the data from the initial table and store it in a vector of partitions
        let mut partitions = vec![];
        for partition in initial_table.batches.iter() {
            let part = partition.read().await.clone();
            partitions.push(part);
        }
        Ok(partitions)
    }

    /// Returns the value of results. For example, returns 6 given the following
    ///
    /// ```text
    /// +-------+,
    /// | count |,
    /// +-------+,
    /// | 6     |,
    /// +-------+,
    /// ```
    fn extract_count(res: Vec<RecordBatch>) -> u64 {
        assert_eq!(res.len(), 1, "expected one batch, got {}", res.len());
        let batch = &res[0];
        assert_eq!(
            batch.num_columns(),
            1,
            "expected 1 column, got {}",
            batch.num_columns()
        );
        let col = batch.column(0).as_primitive::<UInt64Type>();
        assert_eq!(col.len(), 1, "expected 1 row, got {}", col.len());
        let val = col
            .iter()
            .next()
            .expect("had value")
            .expect("expected non null");
        val
    }

    // Test inserting a single batch of data into a single partition
    #[tokio::test]
    async fn test_insert_into_single_partition() -> Result<()> {
        // Create a new schema with one field called "a" of type Int32
        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));

        // Create a new batch of data to insert into the table
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )?;
        // Run the experiment and obtain the resulting data in the table
        let resulting_data_in_table =
            experiment(schema, vec![vec![batch.clone()]], vec![vec![batch.clone()]])
                .await?;
        // Ensure that the table now contains two batches of data in the same partition
        assert_eq!(resulting_data_in_table[0].len(), 2);
        Ok(())
    }

    // Test inserting multiple batches of data into a single partition
    #[tokio::test]
    async fn test_insert_into_single_partition_with_multi_partition() -> Result<()> {
        // Create a new schema with one field called "a" of type Int32
        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));

        // Create a new batch of data to insert into the table
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )?;
        // Run the experiment and obtain the resulting data in the table
        let resulting_data_in_table = experiment(
            schema,
            vec![vec![batch.clone()]],
            vec![vec![batch.clone()], vec![batch]],
        )
        .await?;
        // Ensure that the table now contains three batches of data in the same partition
        assert_eq!(resulting_data_in_table[0].len(), 3);
        Ok(())
    }

    // Test inserting multiple batches of data into multiple partitions
    #[tokio::test]
    async fn test_insert_into_multi_partition_with_multi_partition() -> Result<()> {
        // Create a new schema with one field called "a" of type Int32
        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));

        // Create a new batch of data to insert into the table
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )?;
        // Run the experiment and obtain the resulting data in the table
        let resulting_data_in_table = experiment(
            schema,
            vec![vec![batch.clone()], vec![batch.clone()]],
            vec![
                vec![batch.clone(), batch.clone()],
                vec![batch.clone(), batch],
            ],
        )
        .await?;
        // Ensure that each partition in the table now contains three batches of data
        assert_eq!(resulting_data_in_table[0].len(), 3);
        assert_eq!(resulting_data_in_table[1].len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn test_insert_from_empty_table() -> Result<()> {
        // Create a new schema with one field called "a" of type Int32
        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));

        // Create a new batch of data to insert into the table
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )?;
        // Run the experiment and obtain the resulting data in the table
        let resulting_data_in_table = experiment(
            schema,
            vec![vec![batch.clone(), batch.clone()]],
            vec![vec![]],
        )
        .await?;
        // Ensure that the table now contains two batches of data in the same partition
        assert_eq!(resulting_data_in_table[0].len(), 2);
        Ok(())
    }

    // Test inserting a batch into a MemTable without any partitions
    #[tokio::test]
    async fn test_insert_into_zero_partition() -> Result<()> {
        // Create a new schema with one field called "a" of type Int32
        let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int32, false)]));

        // Create a new batch of data to insert into the table
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )?;
        // Run the experiment and expect an error
        let experiment_result = experiment(schema, vec![], vec![vec![batch.clone()]])
            .await
            .unwrap_err();
        // Ensure that there is a descriptive error message
        assert_eq!(
            "Error during planning: No partitions provided, expected at least one partition",
            experiment_result.strip_backtrace()
        );
        Ok(())
    }
}
