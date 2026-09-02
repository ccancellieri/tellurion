use std::{
    io::{Cursor, Write},
    ops::Range,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};
use tellurion_shapefile::{ArchiveLimits, ArchiveSpool};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

#[derive(Clone)]
struct FixtureObject {
    bytes: Arc<Vec<u8>>,
    handle: SourceHandle,
    identity: ContentIdentity,
    pause: bool,
    requests: Arc<AtomicUsize>,
}

impl FixtureObject {
    fn new(bytes: Vec<u8>) -> Self {
        let length = bytes.len() as u64;
        Self {
            bytes: Arc::new(bytes),
            handle: SourceHandle::new("shapefile-fixture"),
            identity: ContentIdentity::StrongEtag {
                source_key: [1; 32],
                revision_key: [2; 32],
                length,
            },
            pause: false,
            requests: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn paused(bytes: Vec<u8>) -> Self {
        Self {
            pause: true,
            ..Self::new(bytes)
        }
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    fn with_identity(mut self, source_key: [u8; 32], revision_key: [u8; 32]) -> Self {
        self.identity = ContentIdentity::StrongEtag {
            source_key,
            revision_key,
            length: self.bytes.len() as u64,
        };
        self
    }
}

#[async_trait]
impl RangeObject for FixtureObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }

    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }

    fn length(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn display_name(&self) -> &str {
        "fixture.zip"
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if self.pause {
            std::future::pending::<()>().await;
        }
        let start = usize::try_from(range.start).unwrap();
        let end = usize::try_from(range.end).unwrap();
        Ok(Bytes::copy_from_slice(&self.bytes[start..end]))
    }
}

fn archive(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, contents) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn valid_archive(method: CompressionMethod) -> Vec<u8> {
    archive(
        &[
            ("coast/ne.shp", b"shape"),
            ("coast/ne.shx", b"index"),
            ("coast/ne.dbf", b"table"),
            ("coast/ne.prj", b"crs"),
            ("coast/ne.cpg", b"UTF-8"),
        ],
        method,
    )
}

fn stored_local_member(name: &str, contents: &[u8]) -> Vec<u8> {
    let mut member = Vec::new();
    member.extend_from_slice(b"PK\x03\x04");
    member.extend_from_slice(&20_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&crc32fast::hash(contents).to_le_bytes());
    member.extend_from_slice(&u32::try_from(contents.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&u32::try_from(contents.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(name.as_bytes());
    member.extend_from_slice(contents);
    member
}

fn stored_central_member(name: &str, contents: &[u8], local_offset: u32) -> Vec<u8> {
    let mut member = Vec::new();
    member.extend_from_slice(b"PK\x01\x02");
    member.extend_from_slice(&20_u16.to_le_bytes());
    member.extend_from_slice(&20_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&crc32fast::hash(contents).to_le_bytes());
    member.extend_from_slice(&u32::try_from(contents.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&u32::try_from(contents.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u16.to_le_bytes());
    member.extend_from_slice(&0_u32.to_le_bytes());
    member.extend_from_slice(&local_offset.to_le_bytes());
    member.extend_from_slice(name.as_bytes());
    member
}

fn overlapping_local_archive() -> Vec<u8> {
    let shx = stored_local_member("ne.shx", b"index");
    let shp = stored_local_member("ne.shp", &shx);
    let dbf = stored_local_member("ne.dbf", b"table");
    let shx_offset = u32::try_from(30 + "ne.shp".len()).unwrap();
    let dbf_offset = u32::try_from(shp.len()).unwrap();

    let mut archive = shp;
    archive.extend_from_slice(&dbf);
    let central_offset = u32::try_from(archive.len()).unwrap();
    let mut central = Vec::new();
    central.extend_from_slice(&stored_central_member("ne.shp", &shx, 0));
    central.extend_from_slice(&stored_central_member("ne.shx", b"index", shx_offset));
    central.extend_from_slice(&stored_central_member("ne.dbf", b"table", dbf_offset));
    archive.extend_from_slice(&central);
    archive.extend_from_slice(b"PK\x05\x06");
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive.extend_from_slice(&3_u16.to_le_bytes());
    archive.extend_from_slice(&3_u16.to_le_bytes());
    archive.extend_from_slice(&u32::try_from(central.len()).unwrap().to_le_bytes());
    archive.extend_from_slice(&central_offset.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive
}

fn spool(root: &Path) -> ArchiveSpool {
    ArchiveSpool::new(root, ArchiveLimits::default()).unwrap()
}

async fn assert_rejected(bytes: Vec<u8>) {
    let root = tempfile::tempdir().unwrap();
    let error = spool(root.path())
        .materialize(Arc::new(FixtureObject::new(bytes)))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("fixture.zip"));
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn materializes_valid_stored_archive() {
    let root = tempfile::tempdir().unwrap();
    let validated = spool(root.path())
        .materialize(Arc::new(FixtureObject::new(valid_archive(
            CompressionMethod::Stored,
        ))))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&validated.shp).unwrap(), b"shape");
    assert_eq!(std::fs::read(&validated.shx).unwrap(), b"index");
    assert_eq!(std::fs::read(&validated.dbf).unwrap(), b"table");
    assert_eq!(std::fs::read(validated.prj.unwrap()).unwrap(), b"crs");
    assert_eq!(std::fs::read(validated.cpg.unwrap()).unwrap(), b"UTF-8");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(validated.shp.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&validated.shp)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn materializes_valid_deflated_archive() {
    let root = tempfile::tempdir().unwrap();
    let validated = spool(root.path())
        .materialize(Arc::new(FixtureObject::new(valid_archive(
            CompressionMethod::Deflated,
        ))))
        .await
        .unwrap();
    assert!(validated.shp.exists());
}

#[tokio::test]
async fn ignores_safe_regular_auxiliary_members() {
    let root = tempfile::tempdir().unwrap();
    let validated = spool(root.path())
        .materialize(Arc::new(FixtureObject::new(archive(
            &[
                ("ne.shp", b"shape"),
                ("ne.shx", b"index"),
                ("ne.dbf", b"table"),
                ("ne.README.html", b"documentation"),
                ("ne.VERSION.txt", b"1.0"),
            ],
            CompressionMethod::Stored,
        ))))
        .await
        .unwrap();

    let mut extracted = std::fs::read_dir(validated.shp.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    extracted.sort();
    assert_eq!(extracted, ["dataset.dbf", "dataset.shp", "dataset.shx"]);
}

#[tokio::test]
async fn refuses_missing_or_mixed_companions() {
    assert_rejected(archive(
        &[("ne.shp", b"shape"), ("ne.shx", b"index")],
        CompressionMethod::Stored,
    ))
    .await;
    assert_rejected(archive(
        &[
            ("ne.shp", b"shape"),
            ("ne.shx", b"index"),
            ("other.dbf", b"table"),
        ],
        CompressionMethod::Stored,
    ))
    .await;
}

#[tokio::test]
async fn refuses_unsafe_and_ambiguous_member_names() {
    for name in [
        "../ne.shp",
        "/ne.shp",
        "ne\\ne.shp",
        "CON.shp",
        "ne\u{301}.shp",
    ] {
        assert_rejected(archive(
            &[(name, b"shape"), ("ne.shx", b"index"), ("ne.dbf", b"table")],
            CompressionMethod::Stored,
        ))
        .await;
    }
    assert_rejected(archive(
        &[
            ("ne.shp", b"shape"),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
            ("NE.SHP", b"duplicate"),
        ],
        CompressionMethod::Stored,
    ))
    .await;
}

#[test]
fn refuses_configuration_that_weakens_public_bounds() {
    let root = tempfile::tempdir().unwrap();
    for limits in [
        ArchiveLimits {
            max_compressed_bytes: 48 * 1024 * 1024 + 1,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            max_expanded_bytes: 256 * 1024 * 1024 + 1,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            max_members: 33,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            max_ratio: 101,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            max_concurrent: 3,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            max_aggregate_bytes: 512 * 1024 * 1024 + 1,
            ..ArchiveLimits::default()
        },
        ArchiveLimits {
            range_chunk_bytes: 128 * 1024,
            ..ArchiveLimits::default()
        },
    ] {
        assert!(ArchiveSpool::new(root.path(), limits).is_err());
    }
}

#[tokio::test]
async fn refuses_encryption_nested_archives_and_special_files() {
    let mut encrypted = valid_archive(CompressionMethod::Stored);
    set_flag(&mut encrypted, 1);
    assert_rejected(encrypted).await;

    assert_rejected(archive(
        &[
            ("ne.shp", b"shape"),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
            ("nested.zip", b"PK\x03\x04"),
        ],
        CompressionMethod::Stored,
    ))
    .await;

    let mut special = valid_archive(CompressionMethod::Stored);
    set_external_mode(&mut special, 0o120777);
    assert_rejected(special).await;
}

#[tokio::test]
async fn refuses_zip64_member_and_size_bombs() {
    let mut zip64 = valid_archive(CompressionMethod::Stored);
    zip64.extend_from_slice(b"PK\x06\x06zip64");
    assert_rejected(zip64).await;

    let mut declared = valid_archive(CompressionMethod::Stored);
    set_sizes(&mut declared, u32::MAX, u32::MAX);
    assert_rejected(declared).await;

    let root = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        max_expanded_bytes: 32,
        max_ratio: 2,
        ..ArchiveLimits::default()
    };
    let error = ArchiveSpool::new(root.path(), limits)
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(archive(
            &[
                ("ne.shp", &[0; 96]),
                ("ne.shx", b"index"),
                ("ne.dbf", b"table"),
            ],
            CompressionMethod::Deflated,
        ))))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("fixture.zip"));
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());

    let mut actual = archive(
        &[
            ("ne.shp", &[0; 96]),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
        ],
        CompressionMethod::Deflated,
    );
    set_expanded_size(&mut actual, 1);
    let limits = ArchiveLimits {
        max_expanded_bytes: 32,
        ..ArchiveLimits::default()
    };
    let root = tempfile::tempdir().unwrap();
    let error = ArchiveSpool::new(root.path(), limits)
        .unwrap()
        .materialize(Arc::new(FixtureObject::new(actual)))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("fixture.zip"));
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn refuses_eocd_and_local_member_overlap() {
    let mut eocd_overlap = valid_archive(CompressionMethod::Stored);
    let eocd = rfind(&eocd_overlap, b"PK\x05\x06").unwrap();
    let size = u32::from_le_bytes(eocd_overlap[eocd + 12..eocd + 16].try_into().unwrap());
    eocd_overlap[eocd + 12..eocd + 16].copy_from_slice(&(size + 1).to_le_bytes());
    assert_rejected(eocd_overlap).await;

    assert_rejected(overlapping_local_archive()).await;
}

#[tokio::test]
async fn refuses_a_genuine_data_descriptor() {
    let mut descriptor_archive = valid_archive(CompressionMethod::Stored);
    add_data_descriptor_to_first_member(&mut descriptor_archive);
    assert_rejected(descriptor_archive).await;
}

#[tokio::test]
async fn refuses_excessive_members_and_crc_corruption() {
    let mut entries = vec![
        ("ne.shp", b"shape" as &[u8]),
        ("ne.shx", b"index"),
        ("ne.dbf", b"table"),
    ];
    let extra_names = (0..30)
        .map(|index| format!("extra-{index}.txt"))
        .collect::<Vec<_>>();
    for name in &extra_names {
        entries.push((name, b"ignore"));
    }
    assert_rejected(archive(&entries, CompressionMethod::Stored)).await;

    let mut corrupt = valid_archive(CompressionMethod::Stored);
    corrupt_first_member_data(&mut corrupt);
    assert_rejected(corrupt).await;
}

#[tokio::test]
async fn cancellation_removes_partial_spool() {
    let root = tempfile::tempdir().unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(20),
        spool(root.path()).materialize(Arc::new(FixtureObject::paused(valid_archive(
            CompressionMethod::Stored,
        )))),
    )
    .await;
    assert!(result.is_err());
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn reuses_a_live_revision_and_expires_an_idle_entry() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Arc::new(FixtureObject::new(valid_archive(CompressionMethod::Stored)));
    let spool = spool(root.path());
    let first = spool.materialize(fixture.clone()).await.unwrap();
    let reads = fixture.requests();
    let second = spool.materialize(fixture.clone()).await.unwrap();
    assert_eq!(fixture.requests(), reads);
    assert_eq!(first.shp, second.shp);
    drop(first);
    drop(second);

    let root = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        expiry: Duration::ZERO,
        ..ArchiveLimits::default()
    };
    let spool = ArchiveSpool::new(root.path(), limits).unwrap();
    let entry = spool
        .materialize(Arc::new(FixtureObject::new(valid_archive(
            CompressionMethod::Stored,
        ))))
        .await
        .unwrap();
    drop(entry);
    spool.cleanup_expired().await;
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn keeps_same_revision_from_different_sources_separate() {
    let root = tempfile::tempdir().unwrap();
    let spool = spool(root.path());
    let first = FixtureObject::new(archive(
        &[
            ("ne.shp", b"one"),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
        ],
        CompressionMethod::Stored,
    ))
    .with_identity([7; 32], [9; 32]);
    let second = FixtureObject::new(archive(
        &[
            ("ne.shp", b"two"),
            ("ne.shx", b"index"),
            ("ne.dbf", b"table"),
        ],
        CompressionMethod::Stored,
    ))
    .with_identity([8; 32], [9; 32]);
    let first = spool.materialize(Arc::new(first)).await.unwrap();
    let second = spool.materialize(Arc::new(second)).await.unwrap();
    assert_eq!(std::fs::read(&first.shp).unwrap(), b"one");
    assert_eq!(std::fs::read(&second.shp).unwrap(), b"two");
    assert_ne!(first.shp, second.shp);
}

#[tokio::test]
async fn enforces_the_spool_deadline_while_reading() {
    let root = tempfile::tempdir().unwrap();
    let limits = ArchiveLimits {
        deadline: Duration::from_millis(5),
        ..ArchiveLimits::default()
    };
    let result = ArchiveSpool::new(root.path(), limits)
        .unwrap()
        .materialize(Arc::new(FixtureObject::paused(valid_archive(
            CompressionMethod::Stored,
        ))))
        .await;
    assert!(result.is_err());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

fn set_flag(bytes: &mut [u8], flag: u16) {
    for signature in [b"PK\x03\x04".as_slice(), b"PK\x01\x02".as_slice()] {
        let index = find(bytes, signature).unwrap();
        let offset = if signature == b"PK\x03\x04" { 6 } else { 8 };
        bytes[index + offset..index + offset + 2].copy_from_slice(&flag.to_le_bytes());
    }
}

fn set_external_mode(bytes: &mut [u8], mode: u32) {
    let index = find(bytes, b"PK\x01\x02").unwrap();
    bytes[index + 38..index + 42].copy_from_slice(&(mode << 16).to_le_bytes());
}

fn set_sizes(bytes: &mut [u8], compressed: u32, expanded: u32) {
    let local = find(bytes, b"PK\x03\x04").unwrap();
    bytes[local + 18..local + 22].copy_from_slice(&compressed.to_le_bytes());
    bytes[local + 22..local + 26].copy_from_slice(&expanded.to_le_bytes());
    let central = find(bytes, b"PK\x01\x02").unwrap();
    bytes[central + 20..central + 24].copy_from_slice(&compressed.to_le_bytes());
    bytes[central + 24..central + 28].copy_from_slice(&expanded.to_le_bytes());
}

fn set_expanded_size(bytes: &mut [u8], expanded: u32) {
    let local = find(bytes, b"PK\x03\x04").unwrap();
    bytes[local + 22..local + 26].copy_from_slice(&expanded.to_le_bytes());
    let central = find(bytes, b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&expanded.to_le_bytes());
}

fn corrupt_first_member_data(bytes: &mut [u8]) {
    let local = find(bytes, b"PK\x03\x04").unwrap();
    let name_len = u16::from_le_bytes([bytes[local + 26], bytes[local + 27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[local + 28], bytes[local + 29]]) as usize;
    bytes[local + 30 + name_len + extra_len] ^= 1;
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn add_data_descriptor_to_first_member(bytes: &mut Vec<u8>) {
    let local = find(bytes, b"PK\x03\x04").unwrap();
    let central = find(bytes, b"PK\x01\x02").unwrap();
    let eocd = rfind(bytes, b"PK\x05\x06").unwrap();
    let flags = u16::from_le_bytes(bytes[local + 6..local + 8].try_into().unwrap()) | 0b1000;
    let crc = bytes[local + 14..local + 18].to_vec();
    let compressed = bytes[local + 18..local + 22].to_vec();
    let expanded = bytes[local + 22..local + 26].to_vec();
    bytes[local + 6..local + 8].copy_from_slice(&flags.to_le_bytes());
    bytes[central + 8..central + 10].copy_from_slice(&flags.to_le_bytes());
    bytes[local + 14..local + 26].fill(0);
    let name_len = u16::from_le_bytes(bytes[local + 26..local + 28].try_into().unwrap()) as usize;
    let extra_len = u16::from_le_bytes(bytes[local + 28..local + 30].try_into().unwrap()) as usize;
    let data_end = local
        + 30
        + name_len
        + extra_len
        + u32::from_le_bytes(compressed.clone().try_into().unwrap()) as usize;
    let mut descriptor = b"PK\x07\x08".to_vec();
    descriptor.extend_from_slice(&crc);
    descriptor.extend_from_slice(&compressed);
    descriptor.extend_from_slice(&expanded);
    bytes.splice(data_end..data_end, descriptor.iter().copied());
    let shifted_eocd = eocd + descriptor.len();
    let central_offset = u32::from_le_bytes(
        bytes[shifted_eocd + 16..shifted_eocd + 20]
            .try_into()
            .unwrap(),
    );
    bytes[shifted_eocd + 16..shifted_eocd + 20]
        .copy_from_slice(&(central_offset + descriptor.len() as u32).to_le_bytes());
}
