use hunin_runtimes::{
    Limits,
    source_maps::{Binding, BoundMap, digest},
};

#[test]
fn mapping_is_build_bound_and_does_not_fill_unmapped_gaps_or_other_lines() {
    let generated = b"function a(){return 1}";
    let map = br#"{"version":3,"file":"out.js","sources":["original.ts"],"sourcesContent":["export function resize() { return 1 }"],"names":["resize"],"mappings":"AAAAA,K"}"#;
    let binding = Binding {
        generated_sha256: digest(generated),
        map_sha256: digest(map),
    };
    let bound = BoundMap::parse(generated, map, &binding, Limits::default()).unwrap();
    let original = bound.lookup(0, 0).unwrap();
    assert_eq!(original.source, "original.ts");
    assert_eq!(original.line, 0);
    assert_eq!(original.column, 0);
    assert_eq!(original.name.as_deref(), Some("resize"));
    assert!(bound.lookup(0, 5).is_none());
    assert!(bound.lookup(1, 0).is_none());
    assert!(BoundMap::parse(b"another build", map, &binding, Limits::default()).is_err());
    assert!(
        BoundMap::parse(
            generated,
            br#"{"version":3,"sources":[],"names":[],"mappings":""}"#,
            &binding,
            Limits::default()
        )
        .is_err()
    );
    let records = bound.records("script").unwrap();
    let reopened = BoundMap::from_records(&records.records).unwrap();
    assert_eq!(reopened.lookup(0, 0), Some(original));
    assert!(reopened.lookup(0, 8).is_none());
}

#[test]
fn retained_map_lookup_survives_container_removal_and_database_reopen() {
    use hunin_core::{
        project::Project,
        query::{Query, QueryLimits},
        records::SourceDefinition,
    };
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let project = Project::create(directory.path(), "maps").unwrap();
    let investigation = project
        .create_investigation("i", "compiler bindings", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let generated = b"function resize(){return 7}";
    let map=br#"{"version":3,"sources":["preview.ts"],"sourcesContent":["export function resize(){return 7}"],"names":["resize"],"mappings":"AAAAA,K"}"#;
    let raw = project.directory().join("tmp/build.map");
    std::fs::write(&raw, map).unwrap();
    let binding = Binding {
        generated_sha256: digest(generated),
        map_sha256: digest(map),
    };
    let bound = BoundMap::parse(generated, map, &binding, Limits::default()).unwrap();
    let extraction = bound.records("build").unwrap();
    let source = project
        .register_source(
            &run,
            SourceDefinition {
                id: "compiler-map".into(),
                target: None,
                schemas: extraction.schemas,
                clocks: vec![],
                interpretation: "source-map-v3".into(),
                metadata: extraction.metadata,
            },
        )
        .unwrap();
    source.ready(json!({})).unwrap();
    source.append(&extraction.records).unwrap();
    source.close(json!({})).unwrap();
    std::fs::remove_file(&raw).unwrap();
    drop(source);
    drop(project);
    let reopened = Project::open(directory.path(), "maps").unwrap();
    let mut query = Query::new(run);
    query.source = Some("compiler-map".into());
    let page = reopened
        .query(&query, QueryLimits::default(), None)
        .unwrap();
    let records = page
        .records
        .into_iter()
        .map(|entry| entry.record.unwrap())
        .collect::<Vec<_>>();
    let restored = BoundMap::from_records(&records).unwrap();
    assert_eq!(restored.lookup(0, 0), bound.lookup(0, 0));
    assert!(restored.lookup(0, 5).is_none());
}
