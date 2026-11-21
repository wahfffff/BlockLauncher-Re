//! Host-side tests for the whole core.
//!
//! There is no device here, so the two formats that matter — binary XML and
//! ZIP — are *built* in this file. That is deliberate: a test that only reads a
//! fixture cannot prove the reader understands the layout, because the fixture
//! and the reader could share the same mistake. Writing the bytes by hand from
//! the documented layout is what makes these tests worth running.

use blre_core::apk;
use blre_core::axml;
use blre_core::catalog::Library;
use blre_core::launch::{self, Rejection};
use blre_core::nmod::{self, Package};
use blre_core::version::{GameRange, GameVersion};
use blre_core::Error;

use std::path::PathBuf;

// --------------------------------------------------------------- AXML builder

const RES_STRING_POOL: u16 = 0x0001;
const RES_XML: u16 = 0x0003;
const RES_XML_START_ELEMENT: u16 = 0x0102;
const RES_XML_END_ELEMENT: u16 = 0x0103;

const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_BOOLEAN: u8 = 0x12;

#[derive(Clone)]
enum Val {
    Str(u32),
    Int(i32),
    Bool(bool),
}

struct Pool {
    strings: Vec<String>,
}

impl Pool {
    fn new() -> Self {
        // Index 0 is conventionally the empty string.
        Pool {
            strings: vec![String::new()],
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_string());
        (self.strings.len() - 1) as u32
    }

    fn encode(&self) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        for s in &self.strings {
            offsets.push(data.len() as u32);
            let units: Vec<u16> = s.encode_utf16().collect();
            data.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for u in units {
                data.extend_from_slice(&u.to_le_bytes());
            }
            data.extend_from_slice(&0u16.to_le_bytes());
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }

        let header_size: u32 = 28;
        let strings_start = header_size + 4 * self.strings.len() as u32;
        let size = strings_start + data.len() as u32;

        let mut out = Vec::new();
        out.extend_from_slice(&RES_STRING_POOL.to_le_bytes());
        out.extend_from_slice(&(header_size as u16).to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(self.strings.len() as u32).to_le_bytes()); // stringCount
        out.extend_from_slice(&0u32.to_le_bytes()); // styleCount
        out.extend_from_slice(&0u32.to_le_bytes()); // flags: UTF-16
        out.extend_from_slice(&strings_start.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // stylesStart
        for o in offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&data);
        out
    }
}

fn start_element(name: u32, attrs: &[(u32, Val)]) -> Vec<u8> {
    // ResXMLTree_node header is 16 bytes; the attribute extension adds 20.
    let header_size: u16 = 16;
    let attr_start: u16 = 20;
    let attr_size: u16 = 20;
    let size = 16 + 20 + 20 * attrs.len() as u32;

    let mut out = Vec::new();
    out.extend_from_slice(&RES_XML_START_ELEMENT.to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // lineNumber
    out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // comment
    out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // ns
    out.extend_from_slice(&name.to_le_bytes());
    out.extend_from_slice(&attr_start.to_le_bytes());
    out.extend_from_slice(&attr_size.to_le_bytes());
    out.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // idIndex
    out.extend_from_slice(&0u16.to_le_bytes()); // classIndex
    out.extend_from_slice(&0u16.to_le_bytes()); // styleIndex

    for (n, v) in attrs {
        let (dtype, data) = match v {
            Val::Str(i) => (TYPE_STRING, *i),
            Val::Int(i) => (TYPE_INT_DEC, *i as u32),
            Val::Bool(b) => (TYPE_INT_BOOLEAN, u32::from(*b)),
        };
        out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // ns
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // rawValue
        out.extend_from_slice(&8u16.to_le_bytes()); // value size
        out.push(0); // res0
        out.push(dtype);
        out.extend_from_slice(&data.to_le_bytes());
    }
    out
}

fn end_element(name: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&RES_XML_END_ELEMENT.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(&24u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // lineNumber
    out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // comment
    out.extend_from_slice(&0xffff_ffffu32.to_le_bytes()); // ns
    out.extend_from_slice(&name.to_le_bytes());
    out
}

fn reference_value() -> Val {
    let _ = TYPE_REFERENCE;
    Val::Int(0x7f01_0000)
}

/// A manifest shaped like a real legacy MCPE build.
fn sample_manifest(version_code: i32, min_sdk: i32, launcher: &str) -> Vec<u8> {
    let mut p = Pool::new();
    let android = p.intern("http://schemas.android.com/apk/res/android");
    let _ = android;

    let t_manifest = p.intern("manifest");
    let t_application = p.intern("application");
    let t_uses_sdk = p.intern("uses-sdk");
    let t_activity = p.intern("activity");
    let t_action = p.intern("action");
    let t_category = p.intern("category");
    let t_intent = p.intern("intent-filter");

    let a_package = p.intern("package");
    let a_version_code = p.intern("versionCode");
    let a_version_name = p.intern("versionName");
    let a_min_sdk = p.intern("minSdkVersion");
    let a_target_sdk = p.intern("targetSdkVersion");
    let a_name = p.intern("name");
    let a_label = p.intern("label");

    let v_package = p.intern("com.mojang.minecraftpe");
    let v_version_name = p.intern("0.14.0");
    let v_label = p.intern("Minecraft PE");
    let v_launcher = p.intern(launcher);
    let v_main = p.intern("android.intent.action.MAIN");
    let v_launcher_cat = p.intern("android.intent.category.LAUNCHER");

    let mut body = Vec::new();
    body.extend_from_slice(&start_element(
        t_manifest,
        &[
            (a_package, Val::Str(v_package)),
            (a_version_code, Val::Int(version_code)),
            (a_version_name, Val::Str(v_version_name)),
        ],
    ));
    body.extend_from_slice(&start_element(
        t_uses_sdk,
        &[
            (a_min_sdk, Val::Int(min_sdk)),
            (a_target_sdk, Val::Int(min_sdk)),
        ],
    ));
    body.extend_from_slice(&end_element(t_uses_sdk));

    body.extend_from_slice(&start_element(
        t_application,
        &[(a_label, Val::Str(v_label))],
    ));
    body.extend_from_slice(&start_element(
        t_activity,
        &[(a_name, Val::Str(v_launcher))],
    ));
    body.extend_from_slice(&start_element(t_intent, &[]));
    body.extend_from_slice(&start_element(t_action, &[(a_name, Val::Str(v_main))]));
    body.extend_from_slice(&end_element(t_action));
    body.extend_from_slice(&start_element(
        t_category,
        &[(a_name, Val::Str(v_launcher_cat))],
    ));
    body.extend_from_slice(&end_element(t_category));
    body.extend_from_slice(&end_element(t_intent));
    body.extend_from_slice(&end_element(t_activity));
    body.extend_from_slice(&end_element(t_application));
    body.extend_from_slice(&end_element(t_manifest));

    let pool = p.encode();

    let total = 8 + pool.len() + body.len();
    let mut out = Vec::new();
    out.extend_from_slice(&RES_XML.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&pool);
    out.extend_from_slice(&body);
    out
}

// ---------------------------------------------------------------- ZIP builder

fn zip_stored(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();

    for (name, data) in entries {
        let offset = out.len() as u32;
        let nb = name.as_bytes();

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc, unused by the reader
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(nb);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&0u16.to_le_bytes()); // date
        central.extend_from_slice(&0u32.to_le_bytes()); // crc
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(nb);
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);

    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn sample_apk(version_code: i32, min_sdk: i32, abis: &[&str]) -> Vec<u8> {
    let manifest = sample_manifest(version_code, min_sdk, "com.mojang.minecraftpe.MainActivity");
    let mut entries: Vec<(String, Vec<u8>)> = vec![("AndroidManifest.xml".to_string(), manifest)];
    for abi in abis {
        entries.push((
            format!("lib/{abi}/libminecraftpe.so"),
            b"\x7fELF".to_vec(),
        ));
    }
    let refs: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    zip_stored(&refs)
}

// ---------------------------------------------------------------- temp helper

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        let uniq = format!(
            "blre-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        p.push(uniq);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// -------------------------------------------------------------------- AXML

#[test]
fn manifest_reads_the_fields_the_launcher_needs() {
    let raw = sample_manifest(400014, 16, "com.mojang.minecraftpe.MainActivity");
    let m = axml::parse(&raw).expect("manifest should parse");

    assert_eq!(m.package, "com.mojang.minecraftpe");
    assert_eq!(m.version_code, 400014);
    assert_eq!(m.version_name, "0.14.0");
    assert_eq!(m.min_sdk, 16);
    assert_eq!(m.target_sdk, 16);
    assert_eq!(m.label.as_deref(), Some("Minecraft PE"));
    assert_eq!(
        m.launcher_activity.as_deref(),
        Some("com.mojang.minecraftpe.MainActivity")
    );
}

#[test]
fn manifest_without_uses_sdk_defaults_to_one_rather_than_zero() {
    // A build with no <uses-sdk> predates the attribute; the platform says 1.
    // Reporting 0 would make every compatibility check reject the build.
    let mut p = Pool::new();
    let t_manifest = p.intern("manifest");
    let a_package = p.intern("package");
    let v_package = p.intern("com.mojang.minecraftpe");
    let body = {
        let mut b = Vec::new();
        b.extend_from_slice(&start_element(
            t_manifest,
            &[(a_package, Val::Str(v_package))],
        ));
        b.extend_from_slice(&end_element(t_manifest));
        b
    };
    let pool = p.encode();
    let total = 8 + pool.len() + body.len();
    let mut raw = Vec::new();
    raw.extend_from_slice(&RES_XML.to_le_bytes());
    raw.extend_from_slice(&8u16.to_le_bytes());
    raw.extend_from_slice(&(total as u32).to_le_bytes());
    raw.extend_from_slice(&pool);
    raw.extend_from_slice(&body);

    let m = axml::parse(&raw).unwrap();
    assert_eq!(m.min_sdk, 1);
    assert_eq!(m.target_sdk, 1);
}

#[test]
fn manifest_rejects_a_file_that_is_not_binary_xml() {
    let err = axml::parse(b"<?xml version=\"1.0\"?>").unwrap_err();
    assert!(matches!(err, Error::BadManifest(_)), "got {err:?}");
}

#[test]
fn manifest_rejects_missing_package() {
    let mut p = Pool::new();
    let t_manifest = p.intern("manifest");
    let body = {
        let mut b = Vec::new();
        b.extend_from_slice(&start_element(t_manifest, &[]));
        b.extend_from_slice(&end_element(t_manifest));
        b
    };
    let pool = p.encode();
    let total = 8 + pool.len() + body.len();
    let mut raw = Vec::new();
    raw.extend_from_slice(&RES_XML.to_le_bytes());
    raw.extend_from_slice(&8u16.to_le_bytes());
    raw.extend_from_slice(&(total as u32).to_le_bytes());
    raw.extend_from_slice(&pool);
    raw.extend_from_slice(&body);

    assert!(matches!(
        axml::parse(&raw).unwrap_err(),
        Error::UnusableManifest(_)
    ));
}

// --------------------------------------------------------------------- APK

#[test]
fn apk_inspection_reports_package_and_abis() {
    let bytes = sample_apk(400014, 16, &["armeabi-v7a", "x86"]);
    let info = apk::inspect(&bytes).expect("apk should inspect");
    assert_eq!(info.manifest.package, "com.mojang.minecraftpe");
    assert_eq!(info.manifest.version_code, 400014);
    // A 32-bit build sorts ahead of a 32-bit x86 build.
    assert_eq!(info.abis, vec!["armeabi-v7a".to_string(), "x86".to_string()]);
}

#[test]
fn apk_without_native_code_reports_no_abis() {
    let bytes = sample_apk(1, 16, &[]);
    let info = apk::inspect(&bytes).unwrap();
    assert!(info.abis.is_empty());
}

#[test]
fn apk_without_manifest_is_rejected() {
    let bytes = zip_stored(&[("classes.dex", vec![0u8; 4])]);
    assert!(matches!(
        apk::inspect(&bytes).unwrap_err(),
        Error::MissingEntry(_)
    ));
}

#[test]
fn apk_that_is_not_a_zip_is_rejected() {
    assert!(matches!(
        apk::inspect(b"definitely not a zip").unwrap_err(),
        Error::NotZip(_)
    ));
}

// ----------------------------------------------------------------- versions

#[test]
fn versions_compare_numerically_not_lexically() {
    let v = |s| GameVersion::parse(s).unwrap();
    assert!(v("0.14.0").compare(&v("0.9.0")) == std::cmp::Ordering::Greater);
    assert!(v("1.1.5.1").compare(&v("1.1.5")) == std::cmp::Ordering::Greater);
    assert!(v("1.16.201.01").compare(&v("1.16.201.1")) == std::cmp::Ordering::Equal);
    assert_eq!(v("0.15.0b1").compare(&v("0.15.0")), std::cmp::Ordering::Less);
}

#[test]
fn version_parse_rejects_junk() {
    for bad in ["", "abc", "1..2", ".", "1.2.", "-1.0"] {
        assert!(GameVersion::parse(bad).is_none(), "should reject {bad:?}");
    }
}

#[test]
fn game_range_is_inclusive() {
    let r = GameRange {
        min: GameVersion::parse("0.14.0"),
        max: GameVersion::parse("1.1.5"),
    };
    assert!(r.contains(&GameVersion::parse("0.14.0").unwrap()));
    assert!(r.contains(&GameVersion::parse("1.0.0").unwrap()));
    assert!(r.contains(&GameVersion::parse("1.1.5").unwrap()));
    assert!(!r.contains(&GameVersion::parse("0.13.0").unwrap()));
    assert!(!r.contains(&GameVersion::parse("1.2.0").unwrap()));
}

// -------------------------------------------------------------------- nmod

/// A valid nmod package as bytes. The real format is a zip with
/// `nmod_manifest.json` at the root — the shape ModdedMCPE's
/// `NModPackage` reads — so the fixtures are zips, not directories.
fn nmod_zip(name: &str, extra_support: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let manifest = format!(
        r#"{{"uuid":"0f8fad5b-d9cb-469f-a165-70867728950e","name":"{name}",
            "version_name":"1.0","version_code":1,"manifest_version":1,
            "min_sdk_version":1,"target_sdk_version":1,"author":"tester",
            "game_supports":[{{"platform":"android","name":"pe",
            "target_game_versions":["0\\.14\\..*"]{extra_support}}}]}}"#
    );
    let mut entries: Vec<(&str, Vec<u8>)> = vec![(nmod::MANIFEST, manifest.into_bytes())];
    entries.extend(files.iter().map(|(n, d)| (*n, d.clone())));
    zip_stored(&entries)
}

#[test]
fn an_nmod_package_applies_its_overrides() {
    let extra = r#","file_overrides":[{"path":"textures/terrain.png"}]"#;
    let files = vec![("android/pe/assets/textures/terrain.png", b"NEW".to_vec())];
    let pkg = Package::open(nmod_zip("Pack", extra, &files)).unwrap();

    let support = pkg.support_for("0.14.3").unwrap().expect("0.14.3 matches");
    assert!(pkg.support_for("1.0.0").unwrap().is_none());

    let dir = TempDir::new("nmod-apply");
    let root = dir.path();
    std::fs::create_dir_all(root.join("textures")).unwrap();
    std::fs::write(root.join("textures/terrain.png"), b"OLD").unwrap();

    let applied = nmod::apply(&pkg, support, root).unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].kind, "file");
    assert_eq!(std::fs::read(root.join("textures/terrain.png")).unwrap(), b"NEW");
}

#[test]
fn an_nmod_manifest_must_sit_at_the_root() {
    let manifest = br#"{"uuid":"0f8fad5b-d9cb-469f-a165-70867728950e","name":"x",
        "version_name":"1","version_code":1,"manifest_version":1,
        "min_sdk_version":1,"target_sdk_version":1,
        "game_supports":[{"platform":"android","name":"pe","target_game_versions":[".*"]}]}"#;
    let bytes = zip_stored(&[("sub/nmod_manifest.json", manifest.to_vec())]);
    let e = Package::open(bytes).unwrap_err();
    assert!(e.to_string().contains("top level"), "{e}");
}

#[test]
fn nmod_reports_missing_optional_metadata_as_warnings() {
    let pkg = Package::open(nmod_zip("Pack", "", &[])).unwrap();
    // author supplied by the fixture; icon, banner and author_email are not.
    let joined = pkg.warnings().join("; ");
    assert!(joined.contains("author_email"), "{joined}");
    assert!(joined.contains("icon"), "{joined}");
    assert!(joined.contains("banner"), "{joined}");
}

#[test]
fn nmod_selects_native_libraries_for_the_abi() {
    let extra = r#","native_libs":[{"name":"mymod"}]"#;
    let files = vec![
        ("android/pe/arm64-v8a/libmymod.so", b"\x7fELF".to_vec()),
        ("android/pe/armeabi-v7a/libmymod.so", b"\x7fELF".to_vec()),
    ];
    let pkg = Package::open(nmod_zip("Pack", extra, &files)).unwrap();
    let support = pkg.support_for("0.14.2").unwrap().unwrap();
    assert_eq!(
        pkg.native_libs(support, "arm64-v8a"),
        vec!["android/pe/arm64-v8a/libmymod.so".to_string()]
    );
    assert!(pkg.native_libs(support, "x86").is_empty());
    assert_eq!(pkg.shipped_abis(support), vec!["arm64-v8a", "armeabi-v7a"]);
}

// ----------------------------------------------------------------- catalog

#[test]
fn import_is_content_addressed_and_idempotent() {
    let dir = TempDir::new("catalog");
    let mut lib = Library::open(dir.path()).unwrap();

    let apk = sample_apk(400014, 16, &["armeabi-v7a"]);
    let first = lib.import(&apk).unwrap();
    assert!(!first.deduped);
    assert_eq!(lib.versions().len(), 1);
    assert!(lib.apk_path(&first.entry.sha256).exists());

    let second = lib.import(&apk).unwrap();
    assert!(second.deduped);
    assert_eq!(second.entry.id, first.entry.id);
    assert_eq!(lib.versions().len(), 1);
}

#[test]
fn catalog_survives_a_reopen() {
    let dir = TempDir::new("reopen");
    let id = {
        let mut lib = Library::open(dir.path()).unwrap();
        lib.import(&sample_apk(1, 16, &["armeabi-v7a"]))
            .unwrap()
            .entry
            .id
    };
    let lib = Library::open(dir.path()).unwrap();
    assert_eq!(lib.versions().len(), 1);
    assert_eq!(lib.versions()[0].id, id);
}

#[test]
fn remove_deletes_the_payload_and_the_record() {
    let dir = TempDir::new("remove");
    let mut lib = Library::open(dir.path()).unwrap();
    let e = lib
        .import(&sample_apk(2, 16, &["armeabi-v7a"]))
        .unwrap()
        .entry;
    let payload = lib.apk_path(&e.sha256);

    lib.remove(&e.id).unwrap();
    assert!(lib.versions().is_empty());
    assert!(!payload.exists());
    assert!(matches!(lib.remove(&e.id).unwrap_err(), Error::NotFound(_)));
}

#[test]
fn a_corrupt_index_is_reported_not_silently_reset() {
    let dir = TempDir::new("corrupt");
    std::fs::create_dir_all(dir.path().join("apks")).unwrap();
    std::fs::write(dir.path().join("versions.json"), b"{ this is not json").unwrap();
    assert!(matches!(
        Library::open(dir.path()).unwrap_err(),
        Error::Corrupt(_)
    ));
}

#[test]
fn importing_garbage_does_not_touch_the_index() {
    let dir = TempDir::new("garbage");
    let mut lib = Library::open(dir.path()).unwrap();
    let _ = lib.import(b"not an apk").unwrap_err();
    assert!(lib.versions().is_empty());
    // Reopening proves nothing was half-written.
    let again = Library::open(dir.path()).unwrap();
    assert!(again.versions().is_empty());
}

// ------------------------------------------------------------------ launch

/// One nmod package for one ABI, matched against `versions` (a JSON array of
/// Java-style patterns).
fn launch_mod(uuid: &str, versions: &str, abi: &str) -> Package {
    let manifest = format!(
        r#"{{"uuid":"{uuid}","name":"Mod {uuid}","version_name":"1","version_code":1,
            "manifest_version":1,"min_sdk_version":1,"target_sdk_version":1,
            "game_supports":[{{"platform":"android","name":"pe",
            "target_game_versions":{versions},
            "native_libs":[{{"name":"mymod"}}]}}]}}"#
    );
    let owned: Vec<(String, Vec<u8>)> = vec![
        (nmod::MANIFEST.to_string(), manifest.into_bytes()),
        (
            format!("android/pe/{abi}/libmymod.so"),
            b"\x7fELF".to_vec(),
        ),
    ];
    let refs: Vec<(&str, Vec<u8>)> = owned.iter().map(|(n, d)| (n.as_str(), d.clone())).collect();
    Package::open(zip_stored(&refs)).unwrap()
}

#[test]
fn plan_picks_the_device_abi_and_keeps_compatible_mods() {
    let dir = TempDir::new("plan");
    let mut lib = Library::open(dir.path()).unwrap();
    let entry = lib
        .import(&sample_apk(400014, 16, &["armeabi-v7a", "arm64-v8a"]))
        .unwrap()
        .entry;

    let device = vec!["arm64-v8a".to_string(), "armeabi-v7a".to_string()];

    let alpha = launch_mod("0f8fad5b-d9cb-469f-a165-708677289501", r#"["0\\.14\\..*"]"#, "arm64-v8a");
    let beta = launch_mod("0f8fad5b-d9cb-469f-a165-708677289502", r#"["0\\.14\\..*"]"#, "x86");
    let gamma = launch_mod("0f8fad5b-d9cb-469f-a165-708677289503", r#"["1\\.0\\..*"]"#, "arm64-v8a");

    let mods = vec![&alpha, &beta, &gamma];
    let (plan, rejected) = launch::build(&entry, &device, &mods, "/data/inst/1").unwrap();

    assert_eq!(plan.abi, "arm64-v8a");
    assert_eq!(plan.package, "com.mojang.minecraftpe");
    assert_eq!(plan.activity, "com.mojang.minecraftpe.MainActivity");
    assert_eq!(plan.data_dir, "/data/inst/1");
    assert_eq!(
        plan.mods
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["0f8fad5b-d9cb-469f-a165-708677289501"]
    );
    assert_eq!(
        plan.mods[0].libs,
        vec!["android/pe/arm64-v8a/libmymod.so".to_string()]
    );

    assert_eq!(rejected.len(), 2);
    assert!(rejected
        .iter()
        .any(|r| r.id == "0f8fad5b-d9cb-469f-a165-708677289502"
            && matches!(&r.reason, Rejection::Abi { wanted, .. } if wanted == "arm64-v8a")));
    assert!(rejected
        .iter()
        .any(|r| r.id == "0f8fad5b-d9cb-469f-a165-708677289503"
            && matches!(&r.reason, Rejection::Version { wanted, .. } if wanted == "0.14.0")));
}

#[test]
fn plan_refuses_a_build_the_device_cannot_run() {
    let dir = TempDir::new("plan-noabi");
    let mut lib = Library::open(dir.path()).unwrap();
    let entry = lib
        .import(&sample_apk(1, 16, &["armeabi-v7a"]))
        .unwrap()
        .entry;

    let device = vec!["arm64-v8a".to_string()];
    let mods: Vec<&Package> = Vec::new();
    let err = launch::build(&entry, &device, &mods, "/d").unwrap_err();
    assert!(matches!(err, Error::UnusableManifest(_)), "got {err:?}");
}

#[test]
fn plan_is_deterministic_regardless_of_mod_order() {
    let dir = TempDir::new("plan-order");
    let mut lib = Library::open(dir.path()).unwrap();
    let entry = lib
        .import(&sample_apk(1, 16, &["armeabi-v7a"]))
        .unwrap()
        .entry;
    let device = vec!["armeabi-v7a".to_string()];

    // uuids deliberately assorted, so sorting by them is visible.
    let a = launch_mod("0f8fad5b-d9cb-469f-a165-7086772895aa", r#"["0\\.14\\..*"]"#, "armeabi-v7a");
    let z = launch_mod("0f8fad5b-d9cb-469f-a165-7086772895ff", r#"["0\\.14\\..*"]"#, "armeabi-v7a");

    let (p1, _) = launch::build(&entry, &device, &[&a, &z], "/d").unwrap();
    let (p2, _) = launch::build(&entry, &device, &[&z, &a], "/d").unwrap();
    assert_eq!(p1, p2);
    assert_eq!(
        p1.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec![
            "0f8fad5b-d9cb-469f-a165-7086772895aa",
            "0f8fad5b-d9cb-469f-a165-7086772895ff"
        ]
    );
}

#[test]
fn a_zip_on_disk_is_checked_from_its_central_directory() {
    let dir = TempDir::new("has-entry");
    let p = dir.path().join("a.zip");
    std::fs::write(
        &p,
        zip_stored(&[("nmod_manifest.json", b"{}".to_vec())]),
    )
    .unwrap();
    assert!(apk::file_has_entry(&p, "nmod_manifest.json"));
    assert!(!apk::file_has_entry(&p, "something-else.json"));

    // A missing file and a file that is not a zip both answer "no" rather than
    // erroring: the caller is asking whether it should handle this file.
    assert!(!apk::file_has_entry(&dir.path().join("nope.zip"), "nmod_manifest.json"));
    let junk = dir.path().join("junk.zip");
    std::fs::write(&junk, b"not a zip at all").unwrap();
    assert!(!apk::file_has_entry(&junk, "nmod_manifest.json"));
}