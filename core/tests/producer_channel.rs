use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use hunin_core::channel::{
    Acknowledgment, MAX_FRAME_BYTES, ProducerEvent, ProducerListener, ProducerMessage, send,
};
use hunin_core::project::Project;
use hunin_core::records::{
    FieldSchema, FieldType, Record, RecordSchema, SourceDefinition, SourceHandle,
};
use serde_json::json;

const DEADLINE: Duration = Duration::from_secs(3);

fn source(root: &std::path::Path, project: &str) -> SourceHandle {
    let p = Project::create(root, project).unwrap();
    let i = p.create_investigation("i", "preview", json!({})).unwrap();
    let run = p.create_run(&i, "r", json!({})).unwrap();
    p.register_source(
        &run,
        SourceDefinition {
            id: "extension".into(),
            target: None,
            schemas: vec![RecordSchema {
                name: "bytes".into(),
                version: 1,
                description: "driver-measured output bytes".into(),
                fields: BTreeMap::from([(
                    "value".into(),
                    FieldSchema {
                        kind: FieldType::Unsigned,
                        required: true,
                        unit: Some("bytes".into()),
                        meaning: "written output size".into(),
                    },
                )]),
            }],
            clocks: vec![],
            interpretation: "extension-v1".into(),
            metadata: json!({"origin":"external producer"}),
        },
    )
    .unwrap()
}

fn record(id: &str, value: u64) -> Record {
    Record {
        id: id.into(),
        schema: "bytes".into(),
        schema_version: 1,
        subject: "asset-7".into(),
        time: None,
        function: None,
        location: None,
        operation: Some("preview-7".into()),
        payload: json!({"value":value}),
    }
}

fn hello(source: &SourceHandle, credential: &str) -> ProducerMessage {
    ProducerMessage::Hello {
        version: 1,
        project: source.project().id().into(),
        source: source.id().into(),
        credential: credential.into(),
        required_semantics: vec!["structured-records-v1".into()],
    }
}

fn ack(stream: &mut TcpStream) -> Acknowledgment {
    stream.set_read_timeout(Some(DEADLINE)).unwrap();
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).unwrap();
    let size = u32::from_be_bytes(prefix) as usize;
    assert!(size <= MAX_FRAME_BYTES);
    let mut data = vec![0; size];
    stream.read_exact(&mut data).unwrap();
    serde_json::from_slice(&data).unwrap()
}

#[test]
fn fragmented_frames_publish_durably_and_replay_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let source = source(root.path(), "a");
    let listener = ProducerListener::bind(source.clone()).unwrap();
    let address = listener.address().unwrap();
    let auth = hello(&source, listener.credential());
    let server = std::thread::spawn(move || {
        let mut session = listener.accept(DEADLINE).unwrap();
        assert!(matches!(
            session.next(DEADLINE).unwrap(),
            ProducerEvent::Ready(_)
        ));
        assert!(matches!(
            session.next(DEADLINE).unwrap(),
            ProducerEvent::Published(_)
        ));
        assert!(matches!(
            session.next(DEADLINE).unwrap(),
            ProducerEvent::Finished(_)
        ));
    });
    let mut client = TcpStream::connect(address).unwrap();
    let payload = serde_json::to_vec(&auth).unwrap();
    for byte in (payload.len() as u32)
        .to_be_bytes()
        .into_iter()
        .chain(payload)
    {
        client.write_all(&[byte]).unwrap();
    }
    assert!(!ack(&mut client).durable);
    assert!(
        send(
            &mut client,
            &ProducerMessage::Ready {
                coverage: json!({"method":"external driver"})
            },
            DEADLINE
        )
        .unwrap()
        .durable
    );
    let batch = ProducerMessage::Batch {
        delivery: "batch-1".into(),
        records: vec![record("measurement-1", 1234)],
    };
    let accepted = send(&mut client, &batch, DEADLINE).unwrap();
    assert!(accepted.durable);
    assert_eq!(accepted.publication.unwrap().inserted, 1);
    send(
        &mut client,
        &ProducerMessage::Finish {
            outcome: json!({"complete":true}),
        },
        DEADLINE,
    )
    .unwrap();
    server.join().unwrap();
    drop(client);
    drop(source);
    let p = Project::open(root.path(), "a").unwrap();
    let source = p.source(&p.reference("r"), "extension").unwrap();
    let replay = source
        .append_batch("batch-1", &[record("measurement-1", 1234)])
        .unwrap();
    assert_eq!(replay.replayed, 1);
    assert_eq!(replay.inserted, 0);
    assert!(
        source
            .append_batch("batch-1", &[record("measurement-1", 999)])
            .is_err()
    );
    assert!(source.append(&[record("late", 10)]).is_err());
}

#[test]
fn incompatible_session_and_excessive_frame_do_not_close_another_source() {
    let root = tempfile::tempdir().unwrap();
    let a = source(root.path(), "a");
    let b = source(root.path(), "b");
    let listener = ProducerListener::bind(a.clone()).unwrap();
    let address = listener.address().unwrap();
    let foreign = hello(&b, listener.credential());
    let server = std::thread::spawn(move || {
        assert!(listener.accept(DEADLINE).is_err());
        assert!(listener.accept(DEADLINE).is_err());
    });
    let mut client = TcpStream::connect(address).unwrap();
    assert!(send(&mut client, &foreign, DEADLINE).is_err());
    let mut oversized = TcpStream::connect(address).unwrap();
    oversized
        .write_all(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())
        .unwrap();
    server.join().unwrap();
    assert_eq!(b.append(&[record("unaffected", 777)]).unwrap().inserted, 1);
    assert!(!a.status().unwrap().closed);
}

#[test]
fn partial_frame_deadline_does_not_depend_on_borrowed_target_exit() {
    let root = tempfile::tempdir().unwrap();
    let s = source(root.path(), "a");
    let listener = ProducerListener::bind(s).unwrap();
    let address = listener.address().unwrap();
    let server = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        assert!(listener.accept(Duration::from_millis(80)).is_err());
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert!(start.elapsed() < Duration::from_secs(1));
    });
    let mut client = TcpStream::connect(address).unwrap();
    client.write_all(&100u32.to_be_bytes()).unwrap();
    client.write_all(b"{").unwrap();
    server.join().unwrap();
}
