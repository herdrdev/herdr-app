use std::{fs, io, path::Path};

const QUICKJS_BASELINE_FNV64: u64 = 0x3302_116a_b0fc_269c;
// HERDR: 29 with 0033, the cherry-pick of quickjs-ng/quickjs#1613.
const EXPECTED_PATCHES: [(&str, u64); 29] = [
    ("0001-rquickjs-jit.patch", 0x18d7_40cc_6943_30cf),
    ("0002-runtime-feedback.patch", 0xe4c1_ed52_ee5d_d469),
    ("0003-element-layout.patch", 0xa12c_ccd2_7420_e880),
    ("0004-tier1-globals.patch", 0x98f5_54d1_54af_f261),
    ("0005-tier1-constructors.patch", 0x3c31_80b9_bfda_1693),
    ("0006-tier1-regexp.patch", 0x2244_ae9d_c124_6612),
    ("0007-msan-slot-boundary.patch", 0xa9b9_35b0_e775_152e),
    ("0008-tier1-atom-values.patch", 0xbc5e_d1f3_3fef_4069),
    ("0009-tier1-arith-slow.patch", 0x3fd2_4634_ef37_456e),
    ("0010-native-entry-cache.patch", 0xe33fff61a67b0955),
    ("0011-helper-pc-cursor.patch", 0x51eab4f65377affd),
    ("0012-shape-retention.patch", 0xe05663cbae513f56),
    ("0013-feedback-gate.patch", 0x73a90a5552a9178e),
    ("0014-inactive-probes.patch", 0x29ec67631b8faf0b),
    ("0015-cold-object-probes.patch", 0xaa99c90905323834),
    ("0017-shape-generation.patch", 0xb2967edcb6e4f4a5),
    ("0018-inline-frame-recovery.patch", 0xb971d61edccde229),
    ("0019-array-feedback.patch", 0x11c3cb13a8297dd4),
    ("0020-typed-array-guard.patch", 0x5d10_38a3_8bc9_9821),
    ("0021-helper-boundary.patch", 0xdf20_0245_013f_eb89),
    ("0022-inline-refcount.patch", 0x508b_6114_19be_28da),
    ("0023-fast-native-entry.patch", 0xbdee_5442_1fd7_ee3e),
    ("0025-tier1-closures.patch", 0x1bfb_2739_eb74_9885),
    ("0026-tier1-generic-ops.patch", 0xd0d9_cdd3_c58e_eb8e),
    ("0027-tier1-exception-regions.patch", 0xb440_7c8a_4873_a208),
    ("0028-tier1-iteration.patch", 0x40d9_9376_b310_d4d4),
    ("0029-native-call-convention.patch", 0xb9cf_65e5_3bca_852e),
    ("0032-object-fast-paths.patch", 0x3104_b683_7243_9d7e),
    ("0033-promise-capability-oom.patch", 0x9175_9aec_1ba0_8b99),
];
pub(crate) const BASELINE_FILES: [&str; 19] = [
    "api-test.c",
    "builtin-array-fromasync.h",
    "builtin-iterator-zip-keyed.h",
    "builtin-iterator-zip.h",
    "cutils.h",
    "dtoa.c",
    "dtoa.h",
    "libregexp-opcode.h",
    "libregexp.c",
    "libregexp.h",
    "libunicode-table.h",
    "libunicode.c",
    "libunicode.h",
    "list.h",
    "quickjs-atom.h",
    "quickjs-c-atomics.h",
    "quickjs-opcode.h",
    "quickjs.c",
    "quickjs.h",
];
const PATCHED_FILE_FINGERPRINTS: [(&str, u64); 21] = [
    ("api-test.c", 0x7af4_1bd9_e9b2_68b4),
    ("builtin-array-fromasync.h", 0xbfd4_3b62_5abd_4aaf),
    ("builtin-iterator-zip-keyed.h", 0x4d42_cf0c_325c_04b1),
    ("builtin-iterator-zip.h", 0xb222_0daf_80a0_cfd0),
    ("cutils.h", 0x85f1_4868_bfc5_48be),
    ("dtoa.c", 0x1aff_2cc2_d08c_f224),
    ("dtoa.h", 0x602f_733b_bb27_f6e1),
    ("libregexp-opcode.h", 0xcab9_d5af_847e_0e1d),
    ("libregexp.c", 0x22cf_560e_7519_7e35),
    ("libregexp.h", 0x67f0_39e8_b9b2_d548),
    ("libunicode-table.h", 0x274f_4562_d305_7644),
    ("libunicode.c", 0xf461_4418_33a0_a781),
    ("libunicode.h", 0xfd4e_9ecf_f3c5_9c57),
    ("list.h", 0xb337_70f7_b76d_a3d8),
    ("quickjs-atom.h", 0x30b4_9116_b6a2_aa99),
    ("quickjs-c-atomics.h", 0x490b_0f29_f631_3fc0),
    // HERDR: was 0xa291_a4b4_7e88_fa66 before 0033.
    ("quickjs.c", 0x262b_3a79_7a0a_4961),
    ("quickjs-jit.h", 0xcb77_8ada_a891_dd38),
    ("quickjs-jit-helpers.h", 0x7a0c_9a1f_beb2_0014),
    ("quickjs-opcode.h", 0x3d05_cfdf_5cf7_2930),
    ("quickjs.h", 0x4831_2cde_9c2f_a5ee),
];

pub fn apply_patch_set(source_dir: &Path, patch_dir: &Path) -> io::Result<()> {
    let actual = baseline_fingerprint(source_dir)?;
    if actual != QUICKJS_BASELINE_FNV64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "QuickJS baseline mismatch: expected fd0a021 source-set fingerprint \
                 {QUICKJS_BASELINE_FNV64:#018x}, got {actual:#018x}"
            ),
        ));
    }

    let mut patches = fs::read_dir(patch_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    patches.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "patch")
    });
    patches.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    let actual_names = patches
        .iter()
        .map(|path| path.file_name().and_then(|name| name.to_str()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 patch filename"))?;
    let expected_names = EXPECTED_PATCHES
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>();
    if actual_names != expected_names {
        return invalid("QuickJS patch-set manifest mismatch");
    }
    let patch_texts = patches
        .iter()
        .zip(EXPECTED_PATCHES)
        .map(|(patch, (_, expected_digest))| {
            let bytes = fs::read(patch)?;
            // Git may materialize text files with CRLF on Windows.  The
            // manifest pins the canonical LF bytes, so accept only that
            // newline-only checkout transformation; every other byte remains
            // covered by the digest below.
            let canonical = canonicalize_crlf(&bytes);
            if fnv1a64(&canonical) != expected_digest {
                return invalid("QuickJS integration patch digest mismatch");
            }
            std::str::from_utf8(&canonical)
                .map(str::to_owned)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "patch is not UTF-8"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    normalize_baseline_newlines(source_dir)?;
    for patch in &patch_texts {
        apply_unified_patch(source_dir, patch)?;
    }
    for (file, expected) in PATCHED_FILE_FINGERPRINTS {
        let actual = fnv1a64(&fs::read(source_dir.join(file))?);
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "patched {file} fingerprint mismatch: expected {expected:#018x}, \
                     got {actual:#018x}"
                ),
            ));
        }
    }
    Ok(())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_from(0xcbf29ce484222325, bytes)
}

fn fnv1a64_from(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn baseline_fingerprint(source_dir: &Path) -> io::Result<u64> {
    let mut hash = 0xcbf29ce484222325;
    for file in BASELINE_FILES {
        hash = fnv1a64_from(hash, file.as_bytes());
        hash = fnv1a64_from(hash, &[0]);
        let bytes = fs::read(source_dir.join(file))?;
        hash = fnv1a64_from(hash, &canonicalize_crlf(&bytes));
    }
    Ok(hash)
}

fn normalize_baseline_newlines(source_dir: &Path) -> io::Result<()> {
    for file in BASELINE_FILES {
        let path = source_dir.join(file);
        let bytes = fs::read(&path)?;
        let canonical = canonicalize_crlf(&bytes);
        if bytes != canonical {
            fs::write(path, canonical)?;
        }
    }
    Ok(())
}

fn canonicalize_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut canonical = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            index += 1;
        }
        canonical.push(bytes[index]);
        index += 1;
    }
    canonical
}

fn apply_unified_patch(root: &Path, patch: &str) -> io::Result<()> {
    let lines = patch.split_inclusive('\n').collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("diff --git ") {
            index += 1;
            continue;
        }
        index += 1;
        while index < lines.len() && !lines[index].starts_with("--- ") {
            index += 1;
        }
        if index + 1 >= lines.len() || !lines[index + 1].starts_with("+++ ") {
            return invalid("patch file header is incomplete");
        }
        let old_path = patch_path(lines[index].trim_end(), "--- ")?;
        let new_path = patch_path(lines[index + 1].trim_end(), "+++ ")?;
        index += 2;
        let path = new_path.as_deref().or(old_path.as_deref()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "patch has no target path")
        })?;
        if !PATCHED_FILE_FINGERPRINTS
            .iter()
            .any(|(allowed, _)| *allowed == path)
        {
            return invalid("patch targets a file outside the pinned source manifest");
        }
        let original = match old_path {
            Some(_) => fs::read_to_string(root.join(path))?,
            None => String::new(),
        };
        let original_lines = original.split_inclusive('\n').collect::<Vec<_>>();
        let mut output = String::with_capacity(original.len());
        let mut cursor = 0;

        while index < lines.len() && !lines[index].starts_with("diff --git ") {
            if !lines[index].starts_with("@@ ") {
                index += 1;
                continue;
            }
            let (old_start, old_count, new_count) = parse_hunk_header(lines[index])?;
            let target = old_start.saturating_sub(1);
            if target < cursor || target > original_lines.len() {
                return invalid("patch hunk has an invalid or overlapping source range");
            }
            for line in &original_lines[cursor..target] {
                output.push_str(line);
            }
            cursor = target;
            index += 1;
            let mut removed = 0;
            let mut added = 0;
            while index < lines.len()
                && !lines[index].starts_with("@@ ")
                && !lines[index].starts_with("diff --git ")
            {
                let line = lines[index];
                match line.as_bytes().first().copied() {
                    Some(b' ') => {
                        require_source_line(&original_lines, cursor, &line[1..])?;
                        output.push_str(&line[1..]);
                        cursor += 1;
                        removed += 1;
                        added += 1;
                    }
                    Some(b'-') => {
                        require_source_line(&original_lines, cursor, &line[1..])?;
                        cursor += 1;
                        removed += 1;
                    }
                    Some(b'+') => {
                        output.push_str(&line[1..]);
                        added += 1;
                    }
                    Some(b'\\') => {}
                    _ => return invalid("patch hunk contains an invalid line prefix"),
                }
                index += 1;
            }
            if removed != old_count || added != new_count {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "patch hunk at old line {old_start} counted {removed}/{added} lines, expected {old_count}/{new_count}"
                    ),
                ));
            }
        }
        for line in &original_lines[cursor..] {
            output.push_str(line);
        }
        if new_path.is_some() {
            fs::write(root.join(path), output)?;
        } else {
            fs::remove_file(root.join(path))?;
        }
    }
    Ok(())
}

fn patch_path(line: &str, prefix: &str) -> io::Result<Option<String>> {
    let path = line
        .strip_prefix(prefix)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid patch path"))?
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty patch path"))?;
    if path == "/dev/null" {
        return Ok(None);
    }
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unsafe patch path"))?;
    if path.contains('/') || path == "." || path == ".." {
        return invalid("patch paths must name one QuickJS source file");
    }
    Ok(Some(path.to_owned()))
}

fn parse_hunk_header(line: &str) -> io::Result<(usize, usize, usize)> {
    let body = line
        .strip_prefix("@@ -")
        .and_then(|line| line.split_once(" @@").map(|parts| parts.0))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid hunk header"))?;
    let (old, new) = body
        .split_once(" +")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid hunk ranges"))?;
    let (old_start, old_count) = parse_range(old)?;
    let (_, new_count) = parse_range(new)?;
    Ok((old_start, old_count, new_count))
}

fn parse_range(range: &str) -> io::Result<(usize, usize)> {
    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
    Ok((
        start
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid hunk start"))?,
        count
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid hunk count"))?,
    ))
}

fn require_source_line(original: &[&str], cursor: usize, expected: &str) -> io::Result<()> {
    if original.get(cursor).copied() != Some(expected) {
        let nearby = original
            .iter()
            .enumerate()
            .skip(cursor.saturating_sub(256))
            .take(512)
            .find_map(|(index, line)| (*line == expected).then_some(index.saturating_add(1)));
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "patch context mismatch at source line {}: expected {:?}, found {:?}, nearby match {:?}",
                cursor.saturating_add(1),
                expected,
                original.get(cursor).copied(),
                nearby
            ),
        ));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidData, message))
}
