use std::{fs::File, sync::Arc};

use parquet::{
    basic::{LogicalType, Repetition, Type as PhysicalType},
    data_type::{ByteArray, ByteArrayType},
    file::writer::SerializedFileWriter,
    schema::types::Type,
};
use serde_json::json;

use kyler_evals::gaia::dataset::Dataset;

#[test]
fn reads_native_parquet_without_python_or_annotation_exposure() {
    let directory = tempfile::tempdir().unwrap();
    let split = directory.path().join("validation");
    std::fs::create_dir(&split).unwrap();
    let columns = [
        ("task_id", "synthetic-1"),
        ("Question", "Compute the result."),
        ("Level", "2"),
        ("Final answer", "private-reference"),
        ("file_name", ""),
    ];
    let fields = columns
        .iter()
        .map(|(name, _)| {
            Arc::new(
                Type::primitive_type_builder(name, PhysicalType::BYTE_ARRAY)
                    .with_repetition(Repetition::REQUIRED)
                    .with_logical_type(Some(LogicalType::String))
                    .build()
                    .unwrap(),
            )
        })
        .collect();
    let schema = Arc::new(
        Type::group_type_builder("gaia")
            .with_fields(fields)
            .build()
            .unwrap(),
    );
    let mut writer = SerializedFileWriter::new(
        File::create(split.join("metadata.parquet")).unwrap(),
        schema,
        Default::default(),
    )
    .unwrap();
    let mut group = writer.next_row_group().unwrap();
    for (_, value) in columns {
        let mut column = group.next_column().unwrap().unwrap();
        column
            .typed::<ByteArrayType>()
            .write_batch(&[ByteArray::from(value)], None, None)
            .unwrap();
        column.close().unwrap();
    }
    group.close().unwrap();
    writer.close().unwrap();
    let dataset = Dataset::load(directory.path(), "validation").unwrap();
    assert_eq!(dataset.tasks.len(), 1);
    assert_eq!(dataset.tasks[0].level, 2);
    assert_eq!(
        dataset.tasks[0].answer.as_deref(),
        Some("private-reference")
    );
}

#[test]
fn detects_lfs_and_rejects_duplicate_ids_and_attachment_traversal() {
    let directory = tempfile::tempdir().unwrap();
    let split = directory.path().join("validation");
    std::fs::create_dir(&split).unwrap();
    std::fs::write(
        split.join("metadata.parquet"),
        "version https://git-lfs.github.com/spec/v1\noid sha256:deadbeef\nsize 123\n",
    )
    .unwrap();
    let error = Dataset::load(directory.path(), "validation")
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("Git LFS pointer"));
    let task = json!({"task_id":"one","Question":"Synthetic task","Level":1,"Final answer":"secret","file_name":"../secret"});
    std::fs::write(split.join("metadata.jsonl"), format!("{task}\n")).unwrap();
    let dataset = Dataset::load(directory.path(), "validation").unwrap();
    assert!(dataset.attachment(&dataset.tasks[0]).is_err());
    std::fs::write(split.join("metadata.jsonl"), format!("{task}\n{task}\n")).unwrap();
    assert!(Dataset::load(directory.path(), "validation").is_err());
}

#[test]
fn hidden_test_answers_are_not_required() {
    let directory = tempfile::tempdir().unwrap();
    let split = directory.path().join("test");
    std::fs::create_dir(&split).unwrap();
    let task = json!({"task_id":"hidden","Question":"Synthetic task","Level":"3"});
    std::fs::write(split.join("metadata.jsonl"), format!("{task}\n")).unwrap();
    let dataset = Dataset::load(directory.path(), "test").unwrap();
    assert!(dataset.tasks[0].answer.is_none());
    assert!(dataset.select(Some(1), None).is_err());
}
