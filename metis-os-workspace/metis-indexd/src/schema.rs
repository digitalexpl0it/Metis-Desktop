//! Tantivy schema v1.

use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, STORED, STRING, Schema, SchemaBuilder,
    TextFieldIndexing, TextOptions,
};

pub const INDEX_SCHEMA_VERSION: u32 = 1;

#[derive(Clone)]
pub struct IndexFields {
    pub path: Field,
    pub basename: Field,
    pub extension: Field,
    pub mime: Field,
    pub size_u64: Field,
    pub mtime_i64: Field,
    pub dev: Field,
    pub ino: Field,
    pub content_hash: Field,
    pub body: Field,
    pub lang: Field,
    pub category: Field,
    pub tags: Field,
    pub root_id: Field,
}

pub fn build_schema() -> (Schema, IndexFields) {
    let mut builder = SchemaBuilder::new();

    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer("default")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let body_opts = TextOptions::default()
        .set_indexing_options(text_indexing.clone())
        .set_stored();
    let basename_opts = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();

    let path = builder.add_text_field("path", STRING | STORED);
    let basename = builder.add_text_field("basename", basename_opts);
    let extension = builder.add_text_field("extension", STRING | STORED);
    let mime = builder.add_text_field("mime", STRING | STORED);
    let size_u64 = builder.add_u64_field("size_u64", INDEXED | STORED | FAST);
    let mtime_i64 = builder.add_i64_field("mtime_i64", INDEXED | STORED | FAST);
    let dev = builder.add_u64_field("dev", INDEXED | STORED | FAST);
    let ino = builder.add_u64_field("ino", INDEXED | STORED | FAST);
    let content_hash = builder.add_text_field("content_hash", STRING | STORED);
    let body = builder.add_text_field("body", body_opts);
    let lang = builder.add_text_field("lang", STRING | STORED);
    let category = builder.add_text_field("category", STRING | STORED | FAST);
    // Multi-valued exact tags (facet-like via STRING).
    let tags = builder.add_text_field("tags", STRING | STORED | FAST);
    let root_id = builder.add_text_field("root_id", STRING | STORED);

    let schema = builder.build();
    let fields = IndexFields {
        path,
        basename,
        extension,
        mime,
        size_u64,
        mtime_i64,
        dev,
        ino,
        content_hash,
        body,
        lang,
        category,
        tags,
        root_id,
    };
    (schema, fields)
}

pub fn default_index_dir() -> std::path::PathBuf {
    let base = directories::BaseDirs::new()
        .map(|b| b.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("metis")
        .join("index")
        .join(format!("v{INDEX_SCHEMA_VERSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_expected_fields() {
        let (schema, fields) = build_schema();
        assert_eq!(schema.get_field_name(fields.path), "path");
        assert_eq!(schema.get_field_name(fields.body), "body");
        assert_eq!(schema.get_field_name(fields.tags), "tags");
    }
}
