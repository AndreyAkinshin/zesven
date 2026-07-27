//! Where the async API differs from the blocking one, it must say so.
//!
//! Six rounds of review found the same shape of defect over and over: the async
//! half doing something the blocking half does not, quietly. Anti-items counted
//! as files. No volume sizes reported. A folder described with the wrong
//! method. An archive whose signature never reached the disk. Each was found by
//! someone reading the two implementations side by side, one field at a time.
//!
//! This does that mechanically. Every case runs the same work through both, and
//! compares the whole of what comes back rather than the parts someone thought
//! to check. Where the async side genuinely cannot do something, the test
//! asserts it *refuses* - a difference that is declared is a design, and one
//! that is silent is a defect.

#![cfg(all(feature = "async", feature = "lzma2"))]

use std::io::Cursor;

use zesven::read::Archive;
use zesven::write::{EntryMeta, WriteOptions, WriteResult, Writer};
use zesven::{ArchivePath, AsyncArchive, AsyncWriter};

/// A scenario both writers can be driven through.
struct Scenario {
    name: &'static str,
    files: Vec<(&'static str, Vec<u8>)>,
    directories: Vec<&'static str>,
    anti_files: Vec<&'static str>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "one file",
            files: vec![("a.txt", b"HELLO".to_vec())],
            directories: vec![],
            anti_files: vec![],
        },
        Scenario {
            name: "empty entry between others",
            files: vec![
                ("first.bin", b"FIRST".to_vec()),
                ("empty.bin", Vec::new()),
                ("last.bin", b"LAST".to_vec()),
            ],
            directories: vec![],
            anti_files: vec![],
        },
        Scenario {
            name: "directories and removals",
            files: vec![("kept.txt", b"KEPT".to_vec())],
            directories: vec!["dir"],
            anti_files: vec!["gone.txt"],
        },
        Scenario {
            name: "an entry worth compressing",
            files: vec![(
                "big.bin",
                b"a line that repeats often enough to compress\n".repeat(4000),
            )],
            directories: vec![],
            anti_files: vec![],
        },
    ]
}

/// Everything the two writers must agree about, in one comparable shape.
///
/// The archives are not byte-identical - they take different paths through
/// compression - so this is what a caller can observe about them.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    entries_written: usize,
    directories_written: usize,
    total_size: u64,
    volume_count: u32,
    /// Whether the reported size is the real length of the archive.
    size_is_the_archives: bool,
    /// The entry names a reader finds, in order.
    paths: Vec<String>,
    /// Each entry's contents, as extracted.
    contents: Vec<(String, Vec<u8>)>,
}

fn observe(result: &WriteResult, archive: Vec<u8>) -> Observed {
    let size_is_the_archives = result.volume_sizes == vec![archive.len() as u64];

    let mut opened = Archive::open(Cursor::new(archive)).expect("the archive must open");
    let paths: Vec<String> = opened
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_string())
        .collect();
    let files: Vec<String> = opened
        .entries()
        .iter()
        .filter(|e| !e.is_directory)
        .map(|e| e.path.as_str().to_string())
        .collect();
    let contents = files
        .into_iter()
        .map(|path| {
            let data = opened.extract_to_vec(&path).unwrap_or_default();
            (path, data)
        })
        .collect();

    Observed {
        entries_written: result.entries_written,
        directories_written: result.directories_written,
        total_size: result.total_size,
        volume_count: result.volume_count,
        size_is_the_archives,
        paths,
        contents,
    }
}

fn blocking(scenario: &Scenario) -> Observed {
    let mut writer = Writer::create(Cursor::new(Vec::new()))
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    for (path, data) in &scenario.files {
        writer
            .add_bytes(ArchivePath::new(path).unwrap(), data)
            .unwrap();
    }
    for path in &scenario.directories {
        writer
            .add_directory(ArchivePath::new(path).unwrap(), EntryMeta::directory())
            .unwrap();
    }
    for path in &scenario.anti_files {
        writer
            .add_anti_item(ArchivePath::new(path).unwrap())
            .unwrap();
    }
    let (result, sink) = writer.finish_into_inner().unwrap();
    observe(&result, sink.into_inner())
}

async fn asynchronous(scenario: &Scenario) -> Observed {
    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    for (path, data) in &scenario.files {
        writer
            .add_bytes(ArchivePath::new(path).unwrap(), data)
            .await
            .unwrap();
    }
    for path in &scenario.directories {
        writer
            .add_directory(ArchivePath::new(path).unwrap(), EntryMeta::directory())
            .await
            .unwrap();
    }
    for path in &scenario.anti_files {
        writer
            .add_stream(
                ArchivePath::new(path).unwrap(),
                &mut &b""[..],
                EntryMeta::anti_item(),
            )
            .await
            .unwrap();
    }
    let (result, sink) = writer.finish_into_inner().await.unwrap();
    observe(&result, sink.into_inner())
}

/// The two writers must describe the same work the same way.
#[tokio::test]
async fn test_the_writers_agree_about_what_they_wrote() {
    for scenario in scenarios() {
        let blocking = blocking(&scenario);
        let asynchronous = asynchronous(&scenario).await;
        assert_eq!(
            blocking, asynchronous,
            "{}: the writers disagree",
            scenario.name,
        );
    }
}

/// Both writers must produce archives the other's reader accepts.
#[tokio::test]
async fn test_each_writer_produces_what_both_readers_read() {
    let dir = tempfile::TempDir::new().unwrap();
    let payload = b"read by whichever half the caller happens to be using\n".repeat(100);

    let blocking_path = dir.path().join("blocking.7z");
    let mut writer = Writer::create_path(&blocking_path)
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), &payload)
        .unwrap();
    let blocking_result = writer.finish().unwrap();

    let async_path = dir.path().join("async.7z");
    let mut writer = AsyncWriter::create_path(&async_path)
        .await
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("a.bin").unwrap(), &payload)
        .await
        .unwrap();
    let async_result = writer.finish().await.unwrap();

    // Both wrote a file, and both must have said how long it is.
    for (result, path) in [
        (&blocking_result, &blocking_path),
        (&async_result, &async_path),
    ] {
        let on_disk = std::fs::metadata(path).unwrap().len();
        assert_eq!(
            result.volume_sizes,
            vec![on_disk],
            "{} is {on_disk} bytes, reported as {:?}",
            path.display(),
            result.volume_sizes,
        );
    }

    for path in [&blocking_path, &async_path] {
        let mut opened = Archive::open_path(path).unwrap();
        assert_eq!(
            opened.extract_to_vec("a.bin").unwrap(),
            payload,
            "the blocking reader could not read {}",
            path.display(),
        );

        let out = dir.path().join(format!(
            "out-{}",
            path.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&out).unwrap();
        let mut opened = AsyncArchive::open_path(path).await.unwrap();
        let _ = opened
            .extract(&out, (), &zesven::AsyncExtractOptions::default())
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(out.join("a.bin")).unwrap(),
            payload,
            "the async reader could not read {}",
            path.display(),
        );
    }
}

/// What the async writer cannot do, it must refuse rather than approximate.
///
/// Listed here as well as in the writer's own tests, because this is the file
/// that says what "the same as the blocking one" means: anything absent from
/// this list and from the parity cases above is a difference nobody declared.
#[tokio::test]
async fn test_the_async_writer_declares_what_it_cannot_do() {
    use zesven::WriteFilter;

    let unsupported = [
        ("filter", WriteOptions::new().filter(WriteFilter::delta(4))),
        ("solid", WriteOptions::new().solid()),
        ("comment", WriteOptions::new().comment("hello")),
    ];

    for (name, options) in unsupported {
        let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
            .await
            .unwrap()
            .options(options);
        assert!(
            writer
                .add_bytes(ArchivePath::new("a.bin").unwrap(), b"DATA")
                .await
                .is_err(),
            "{name} is accepted by the async writer and applied by neither",
        );
    }
}

/// The async reader must refuse what it cannot read, not misread it.
///
/// It opens one file: a multi-volume set and an SFX archive both begin
/// somewhere other than where it looks. Until it handles them, the failure has
/// to be an error rather than a wrong answer, and the documentation has to say
/// so - which is what these two cases pin down.
#[tokio::test]
async fn test_the_async_reader_fails_loudly_on_what_it_cannot_read() {
    use zesven::VolumeConfig;

    let dir = tempfile::TempDir::new().unwrap();

    // Incompressible, so the archive really spans volumes.
    let mut payload = vec![0u8; 400_000];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for byte in payload.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }

    let config = VolumeConfig::new(dir.path().join("multi.7z"), 64 * 1024);
    let mut writer = Writer::create_multivolume(config)
        .unwrap()
        .options(WriteOptions::new().level(1).unwrap());
    writer
        .add_bytes(ArchivePath::new("payload.bin").unwrap(), &payload)
        .unwrap();
    let result = writer.finish().unwrap();
    assert!(result.volume_count > 1);

    let first = dir.path().join("multi.7z.001");

    // The blocking reader reads the set through.
    let mut opened = Archive::open_path(&first).unwrap();
    assert_eq!(opened.extract_to_vec("payload.bin").unwrap(), payload);

    // The async one is handed the same path and must not pretend to succeed.
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let async_result = match AsyncArchive::open_path(&first).await {
        Err(_) => Err(()),
        Ok(mut archive) => archive
            .extract(&out, (), &zesven::AsyncExtractOptions::default())
            .await
            .map(|_| ())
            .map_err(|_| ()),
    };
    assert!(
        async_result.is_err(),
        "the async reader claimed to read a volume set it only saw one file of",
    );
}

/// Nothing the writer can decide up front is decided after reading.
///
/// `add_stream` consumes its source whole before compressing any of it, so a
/// check made afterwards costs the caller the entire read - and, for a source
/// that cannot be rewound, the data itself. Both a method this build cannot
/// use and an entry arriving out of order under `deterministic` are known
/// before a byte is needed.
#[tokio::test]
async fn test_the_async_writer_refuses_out_of_order_entries_before_reading() {
    /// Reports whether anything was read from it.
    struct CountingSource<'a> {
        data: &'a [u8],
        read: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl tokio::io::AsyncRead for CountingSource<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.read.store(true, std::sync::atomic::Ordering::SeqCst);
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            std::task::Poll::Ready(Ok(()))
        }
    }

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().deterministic(true));
    writer
        .add_bytes(ArchivePath::new("z.bin").unwrap(), b"LAST")
        .await
        .unwrap();

    let read = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let payload = b"a stream that cannot be rewound\n".repeat(50);
    let refused = writer
        .add_stream(
            ArchivePath::new("a.bin").unwrap(),
            CountingSource {
                data: &payload,
                read: read.clone(),
            },
            zesven::write::EntryMeta::file(payload.len() as u64),
        )
        .await;

    assert!(
        refused.is_err(),
        "'a.bin' sorts before an entry already added"
    );
    assert!(
        !read.load(std::sync::atomic::Ordering::SeqCst),
        "the source was consumed before the order was checked",
    );
}

/// The same, for a method this build cannot use.
#[tokio::test]
async fn test_the_async_writer_validates_before_reading_its_source() {
    let unavailable = [
        zesven::codec::CodecMethod::Zstd,
        zesven::codec::CodecMethod::Brotli,
        zesven::codec::CodecMethod::Lz4,
        zesven::codec::CodecMethod::PPMd,
    ]
    .into_iter()
    .find(|m| !m.is_available());

    let Some(method) = unavailable else {
        return; // Every codec is compiled in; nothing to check here.
    };

    /// Reports whether anything was read from it.
    struct CountingSource<'a> {
        data: &'a [u8],
        read: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl tokio::io::AsyncRead for CountingSource<'_> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.read.store(true, std::sync::atomic::Ordering::SeqCst);
            let n = self.data.len().min(buf.remaining());
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
            std::task::Poll::Ready(Ok(()))
        }
    }

    let read = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let payload = b"a source that should never be touched\n".repeat(100);

    let mut writer = AsyncWriter::create(Cursor::new(Vec::new()))
        .await
        .unwrap()
        .options(WriteOptions::new().method(method));
    let refused = writer
        .add_stream(
            ArchivePath::new("a.bin").unwrap(),
            CountingSource {
                data: &payload,
                read: read.clone(),
            },
            zesven::write::EntryMeta::file(payload.len() as u64),
        )
        .await;

    assert!(
        refused.is_err(),
        "{method:?} is not available in this build"
    );
    assert!(
        !read.load(std::sync::atomic::Ordering::SeqCst),
        "the source was consumed before the method was checked",
    );
}
