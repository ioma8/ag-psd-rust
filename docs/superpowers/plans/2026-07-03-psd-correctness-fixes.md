# PSD Parser/Writer Correctness Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the correctness bugs found in the 2026-07-03 audit: tagged-block padding convention (12 red tests), Txt2 raw preservation, Unicode string corruption, broken PSB read/write, PSB RLE row counts, non-RGB write guard, and two hardening items.

**Architecture:** `psd-great` is a Rust port of a TypeScript PSD parser. The TS source at `/Users/jakubkolcar/projects/customs/photoshop/psd/src/psd/` is itself a port of Photoshop's own code and is the **interop oracle** — when the Adobe spec text and the TS code disagree, the TS code wins. Byte-exact sample roundtrip tests in `tests/integration_test.rs` and `tests/ts_parity_test.rs` are the regression gate for every task.

**Tech Stack:** Rust, `byteorder`, `flate2`, `cargo test`. Sample PSD corpus at `/Users/jakubkolcar/projects/customs/photoshop/psd/samples/`.

## Global Constraints

- Repo: `/Users/jakubkolcar/projects/customs/ag-psd-rust`, branch `codex/fix-psd-roundtrip-interop`.
- The working tree starts with **uncommitted WIP** and **12 failing tests** (6 in `src/format/additional_info.rs` unit tests, 4 in `tests/integration_test.rs`, 2 in `tests/ts_parity_test.rs`). Task 0 snapshots the WIP; later tasks must only reduce the failure count, never grow it.
- After every task: `cargo test 2>&1 | tail -5` — the byte-exact sample roundtrip tests (`tests/integration_test.rs`, `tests/ts_parity_test.rs`) must not regress.
- Do not commit `tmp/`, `.DS_Store`, `writer-smoke.psd`, or anything under `target/`.
- Commit messages end with: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`

## Ground-truth reference (established during audit — do not re-litigate)

**Layer tagged-block framing** (TS oracle `tagged-block-reader.ts:288-` and `tagged-block-writer.ts:1032-1040`):
- Stored `length` field = content length, **excluding** padding.
- Data is padded to **4-byte** alignment after the content.
- Reader: `seek(dataStart + dataLength)` then `skip((4 - dataLength % 4) % 4)`.
- Verified against real Photoshop samples: `text.psd` has a Txt2 block with `blockLen = 33122`, `rich-text.psd` has `blockLen = 34054` — odd/unaligned lengths followed by pad bytes.

**Txt2 payload**: starts **directly** with engine data (`" /98 << ..."`). There is **no inner u32 length prefix** in real files (verified by hexdump of 6 sample files; TS `parseTextEngineData(raw)` parses from offset 0). The existing unit test `txt2_writes_inner_length_prefix` encodes a wrong expectation and must be replaced.

**luni**: u32 UTF-16 code-unit count + that many u16s. TS writes `value.length` (UTF-16 units, no trailing NUL).

**PSB (version 2) section length widths** (Adobe spec, confirmed by the writer side of this crate, which is already correct):
- 8-byte lengths: layer-and-mask-info section, layer-info sub-section, channel lengths, `8B64` tagged blocks.
- 4-byte lengths everywhere else: color mode data, image resources, global layer mask info, layer-record extra data, layer mask data.
- PSB RLE row byte-counts are 4 bytes per row (2 bytes in PSD).

---

### Task 0: Snapshot the existing WIP

**Files:**
- Modify: none (git only)

- [ ] **Step 1: Commit the uncommitted WIP so later tasks produce clean diffs**

```bash
cd /Users/jakubkolcar/projects/customs/ag-psd-rust
git add src/ tests/ examples/resave_psd.rs examples/create_writer_smoke_psd.rs
git commit -m "wip: tagged-block padding rework and TySh/Txt2 raw capture (12 tests red)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

- [ ] **Step 2: Record the baseline failure list**

Run: `cargo test 2>&1 | grep -E "^test .* FAILED|failures:" | sort -u`
Expected: 6 lib failures + 4 integration + 2 ts_parity. Save this list; it is the scoreboard for Tasks 1–3.

---

### Task 1: Tagged-block length/padding convention (writer + reader)

**Files:**
- Modify: `src/format/additional_info.rs:3451-3481` (`write_tagged_block`), `src/format/additional_info.rs:3383-3425` (padding skip in `read_layer_additional_info`), call sites at `src/format/additional_info.rs:3562,3568,3577,3587` (and the equivalent lines in the document-level path in the same function)
- Test: `src/format/additional_info.rs` unit tests `unknown_tagged_block_roundtrips_via_raw_blocks` (line ~4491), `duplicate_unknown_tagged_blocks_preserve_multiplicity_and_order` (~4516), `mixed_modeled_and_unknown_tagged_blocks_preserve_original_order` (~4534), `layer_tagged_blocks_are_even_padded_on_write` (~4552)

**Interfaces:**
- Produces: `fn write_tagged_block(writer: &mut PsdWriter, key: &str, data: &[u8], large: bool) -> Result<()>` — the `padding: usize` parameter is **removed**; padding is always to 4 bytes, stored length always excludes padding.

- [ ] **Step 1: Update the stale unit tests to the oracle convention**

The convention: stored length = content length; pad to 4 after. Rewrite the four tests. Note `write_tagged_block` loses its last parameter:

```rust
#[test]
fn unknown_tagged_block_roundtrips_via_raw_blocks() {
    let mut writer = PsdWriter::new(64);
    write_tagged_block(&mut writer, "ZZZ1", &[1, 2, 3, 4, 5], false).unwrap();
    let bytes = writer.into_buffer();
    // 8BIM + ZZZ1 + len(5) + 5 data bytes + 3 pad bytes = 20
    assert_eq!(bytes.len(), 20);
    assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), 5);
    assert_eq!(&bytes[17..20], &[0, 0, 0]);

    let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
    let parsed = read_layer_additional_info(&mut reader, bytes.len()).unwrap();
    assert_eq!(parsed.raw_blocks.len(), 1);
    assert_eq!(parsed.raw_blocks[0].key, "ZZZ1");
    assert_eq!(parsed.raw_blocks[0].data, vec![1, 2, 3, 4, 5]);

    let mut rewritten = PsdWriter::new(64);
    write_layer_additional_info(&mut rewritten, &parsed).unwrap();
    assert_eq!(rewritten.into_buffer(), bytes);
}
```

`duplicate_unknown_tagged_blocks_preserve_multiplicity_and_order` and `mixed_modeled_and_unknown_tagged_blocks_preserve_original_order`: only change the `write_tagged_block(...)` calls to drop the trailing `, 2` argument (byte-exact assertions stay as-is — both sides use the new convention).

Rename `layer_tagged_blocks_are_even_padded_on_write` to `layer_tagged_blocks_are_four_byte_padded_on_write` and drop the `, 2` arguments in it; the equality assertion stays.

Keep `test_layer_tagged_blocks_are_padded_to_four_bytes` (line ~3890) untouched — it already encodes the oracle convention (luni length 6, two pad bytes before `8BIMlyid`).

- [ ] **Step 2: Run the updated tests to verify they fail against current code**

Run: `cargo test --lib additional_info::tests -- unknown_tagged duplicate_unknown mixed_modeled four_byte_padded padded_to_four 2>&1 | tail -8`
Expected: FAIL (current writer stores length including padding).

- [ ] **Step 3: Fix `write_tagged_block`**

Replace the body at `src/format/additional_info.rs:3451`:

```rust
fn write_tagged_block(
    writer: &mut PsdWriter,
    key: &str,
    data: &[u8],
    large: bool,
) -> Result<()> {
    let signature = if tagged_block_uses_u64_length(key, large) {
        "8B64"
    } else {
        "8BIM"
    };
    writer.write_signature(signature)?;
    writer.write_signature(key)?;
    if signature == "8B64" {
        writer.write_u32(0)?;
    }
    // Stored length excludes padding; data is padded to 4 bytes (matches
    // the TS oracle tagged-block-writer.ts and real Photoshop files).
    writer.write_u32(data.len() as u32)?;
    writer.write_bytes(data)?;
    let pad = (4 - (data.len() % 4)) % 4;
    if pad != 0 {
        writer.write_zeros(pad)?;
    }
    Ok(())
}
```

Update every call site: `grep -n "write_tagged_block(" src/format/additional_info.rs` and remove the final padding argument from each (production code and tests).

- [ ] **Step 4: Fix the reader's padding skip**

In `read_layer_additional_info` (src/format/additional_info.rs), capture the block start before dispatching, then replace the signature-probing loop (current lines ~3383-3425) with a deterministic seek + skip:

```rust
        let block_start = reader.offset;
        reader.read_additional_info(&key, data_length, &mut info)?;

        // Handlers may under-read; the block length is authoritative.
        reader.seek_to(block_start + data_length as u64)?;

        let consumed = reader.offset.saturating_sub(start_offset);
        if consumed >= length as u64 {
            break;
        }
        // Data is padded to 4 bytes; length field excludes padding.
        let pad = (4 - (data_length % 4)) % 4;
        let remaining = (length as u64 - consumed) as usize;
        reader.skip_bytes(pad.min(remaining))?;
```

Contingency: if Step 5's corpus tests fail on a specific sample after this change, that sample uses non-standard padding — reinstate a *fallback only* (after the deterministic skip, if ≥12 bytes remain and the next 4 bytes are not `8BIM`/`8B64`, probe 1–3 extra bytes). Do not make probing the primary path.

- [ ] **Step 5: Run the full lib + integration suites**

Run: `cargo test 2>&1 | tail -6`
Expected: the 5 padding-related lib tests PASS. Compare total failures against the Task 0 baseline — several integration failures (`default_roundtrip_preserves_pixels_and_structure` lyid loss, `roundtrip_annotation_tagged_block` EOF) are likely caused by this same bug and may now pass. Record the new failure list.

- [ ] **Step 6: Commit**

```bash
git add src/format/additional_info.rs
git commit -m "fix: layer tagged blocks store content length and pad to 4 bytes

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Txt2 raw preservation, no synthetic inner prefix

**Files:**
- Modify: `src/format/additional_info.rs` — `TextEngineBlock` struct (~line 298), `Txt2` read arm (~line 1164), `Txt2` write arm (~line 3149), `apply_text_prewrite` in `src/io/writer.rs:1317`
- Test: replace `txt2_writes_inner_length_prefix` (~line 4576) in `src/format/additional_info.rs`

**Interfaces:**
- Produces: `pub struct TextEngineBlock { pub data: EngineValue, pub raw: Option<Vec<u8>> }` — `raw` is `Some` when the block came from a file and must be written back verbatim.

- [ ] **Step 1: Write the failing tests**

Replace `txt2_writes_inner_length_prefix` with:

```rust
#[test]
fn txt2_preserves_raw_bytes_verbatim() {
    // Real Txt2 payloads start directly with engine data — no inner prefix.
    let raw = b" << /0 1 /1 [ 1.0 ] >> \0\0".to_vec();
    let mut info = LayerAdditionalInfo::default();
    let parsed = crate::support::engine_data::parse_engine_data(&raw).unwrap();
    info.text_engine = Some(TextEngineBlock { data: parsed, raw: Some(raw.clone()) });

    let mut w = PsdWriter::new(256);
    let len = w.write_additional_info("Txt2", &info).unwrap();
    assert_eq!(w.into_buffer()[..len], raw[..]);
}

#[test]
fn txt2_synthesized_has_no_inner_length_prefix() {
    use std::collections::HashMap;
    let engine = crate::support::engine_data::EngineValue::Object(HashMap::from([(
        "_DocumentObjects".to_string(),
        crate::support::engine_data::EngineValue::Object(HashMap::new()),
    )]));
    let mut info = LayerAdditionalInfo::default();
    info.text_engine = Some(TextEngineBlock { data: engine, raw: None });

    let mut w = PsdWriter::new(256);
    let len = w.write_additional_info("Txt2", &info).unwrap();
    let buf = w.into_buffer();
    // Engine data begins with the serializer's leading space, not a u32 prefix.
    assert!(len > 0);
    assert_eq!(buf[0], b' ');
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib txt2 2>&1 | tail -5`
Expected: FAIL — `TextEngineBlock` has no `raw` field (compile error is the failure here).

- [ ] **Step 3: Implement**

1. Add `pub raw: Option<Vec<u8>>` to `TextEngineBlock` (~line 298). Fix all construction sites: the `Txt2` read arm sets `raw: Some(raw_bytes_read)`, `apply_text_prewrite` in `src/io/writer.rs` sets `raw: None`, and any test constructors set `raw: None`. Find them all: `grep -rn "TextEngineBlock {" src/ tests/`.
2. In the `Txt2` read arm (~line 1164), keep the existing parse-with-fallback logic, but store the full raw payload: `info.text_engine = Some(TextEngineBlock { data: parsed, raw: Some(raw) });`
3. In the `Txt2` write arm (~line 3149):

```rust
"Txt2" => {
    if let Some(ref text_engine) = info.text_engine {
        if let Some(ref raw) = text_engine.raw {
            temp_writer.write_bytes(raw)?;
        } else {
            let bytes = crate::support::engine_data::serialize_engine_data(&text_engine.data, true)
                .map_err(|e| PsdError::InvalidFormat(e.to_string()))?;
            temp_writer.write_bytes(&bytes)?;
        }
    }
}
```

Note: `TextEngineBlock` derives `PartialEq`; the integration test `existing_txt2_block_is_not_rewritten_on_roundtrip` compares written bytes, so including `raw` in equality is fine.

- [ ] **Step 4: Run lib tests + the two Txt2 integration tests**

Run: `cargo test --lib txt2 2>&1 | tail -5 && cargo test --test integration_test txt2 2>&1 | tail -5`
Expected: PASS, including `existing_txt2_block_is_not_rewritten_on_roundtrip`.

- [ ] **Step 5: Commit**

```bash
git add src/format/additional_info.rs src/io/writer.rs
git commit -m "fix: preserve Txt2 raw bytes; drop synthetic inner length prefix

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Drive remaining test failures to zero

**Files:**
- Modify: whatever the diagnosis points at (likely `src/format/additional_info.rs` TySh write arm ~line 2569, or none at all)
- Test: existing suites only

This task is diagnostic. **REQUIRED SUB-SKILL:** use superpowers:systematic-debugging for each remaining failure.

- [ ] **Step 1: Get the current failure list**

Run: `cargo test 2>&1 | grep -E "FAILED|failures:" | sort -u`
Expected: most or all of the Task 0 baseline is now green. Candidates that may remain: `tysh_semantic_rewrite_preserves_sample_text_layer_bytes`, `all_sample_psds_remain_semantically_stable_across_multiple_roundtrips`, `default_roundtrip_preserves_pixels_and_structure`, ts_parity failures.

- [ ] **Step 2: For each remaining failure, diagnose before fixing**

Per failure: run it alone with backtrace, e.g.
`RUST_BACKTRACE=1 cargo test --test integration_test tysh_semantic 2>&1 | tail -30`

Known starting hypotheses from the audit:
- **TySh byte diffs**: the read arm (`src/format/additional_info.rs:981-987`) captures `raw_bytes` via a sub-reader over a copied buffer; the write arm (~2569) emits `raw_bytes` verbatim. A byte diff means the *semantic* rewrite path is taken (raw_bytes stripped by the test) and `write_version_and_descriptor` vs the new split `descriptor_version`/`warp_descriptor_version` fields disagree with the file. Compare the first diverging byte offset against a hexdump of the original block.
- **lyid loss / annotation EOF**: were padding-skip casualties; if still failing after Task 1, the deterministic `seek_to` may have exposed a handler that reads *past* its block into the next one — find it by logging `key` when `reader.offset > block_start + data_length` before the `seek_to`.

- [ ] **Step 3: Fix each with a minimal diff, running the single test between fixes**

- [ ] **Step 4: Full suite green check**

Run: `cargo test 2>&1 | tail -4`
Expected: `0 failed` in lib, integration_test, and ts_parity_test (1 pre-existing `#[ignore]` is fine).

- [ ] **Step 5: Commit**

```bash
git add -A -- src/ tests/
git commit -m "fix: remaining roundtrip regressions from padding rework

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Unicode string encode/decode (non-ASCII corruption)

**Files:**
- Modify: `src/io/writer.rs:186-207` (`write_unicode_string`, `write_unicode_string_with_padding`), `src/io/reader.rs:181-195` (`read_unicode_string_with_length`), `src/format/additional_info.rs` `read_unicode_layer_name` (~line 1653: delete the now-redundant trailing-NUL pop)
- Test: `tests/integration_test.rs` (new test at end of file)

**Interfaces:**
- Consumes/Produces: same public signatures; only behavior changes. Length prefix = UTF-16 **code-unit** count; surrogate pairs encoded/decoded correctly; reader strips exactly one trailing NUL.

- [ ] **Step 1: Write the failing test** (append to `tests/integration_test.rs`)

```rust
#[test]
fn non_ascii_layer_names_roundtrip() {
    use std::io::Cursor;
    let psd = Psd {
        width: 1,
        height: 1,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData { data: vec![1, 2, 3, 4], width: 1, height: 1 }),
            additional_info: psd_great::additional_info::LayerAdditionalInfo {
                name: Some("Žluťoučký 😀".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }]),
        ..Default::default()
    };
    let bytes = write_psd(&psd, &WriteOptions::default()).unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!(
        read.children.unwrap()[0].additional_info.name.as_deref(),
        Some("Žluťoučký 😀")
    );
}
```

(Match the existing test file's import style — it already imports `Psd`, `Layer`, `PixelData`, `write_psd`, `read_psd`, `WriteOptions`, `ReadOptions`, `ColorMode` at the top.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test integration_test non_ascii_layer_names 2>&1 | tail -5`
Expected: FAIL — audit repro produced `"Žluťoučký \u{f600}\u{1}\u{2}\u{4}..."`.

- [ ] **Step 3: Implement the writer fix** (`src/io/writer.rs:186-207`)

```rust
    /// Write a Unicode string (UTF-16 BE). Length prefix counts UTF-16 code units.
    pub fn write_unicode_string(&mut self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.write_u32(units.len() as u32)?;
        for unit in units {
            self.write_u16(unit)?;
        }
        Ok(())
    }

    /// Write a Unicode string with a trailing NUL included in the count.
    pub fn write_unicode_string_with_padding(&mut self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.write_u32((units.len() + 1) as u32)?;
        for unit in units {
            self.write_u16(unit)?;
        }
        self.write_u16(0)?;
        Ok(())
    }
```

- [ ] **Step 4: Implement the reader fix** (`src/io/reader.rs:181-195`)

```rust
    /// Read a Unicode string with known length (UTF-16 code units).
    /// Strips exactly one trailing NUL (Photoshop terminator), preserves
    /// interior NULs and decodes surrogate pairs.
    pub fn read_unicode_string_with_length(&mut self, length: usize) -> Result<String> {
        let mut units = Vec::with_capacity(length);
        for _ in 0..length {
            units.push(self.read_u16()?);
        }
        if units.last() == Some(&0) {
            units.pop();
        }
        Ok(String::from_utf16_lossy(&units))
    }
```

Then delete the trailing-NUL pop inside `read_unicode_layer_name` in `src/format/additional_info.rs` (~line 1653) — the helper now handles it:

```rust
    fn read_unicode_layer_name(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.name = Some(self.read_unicode_string()?);
        Ok(())
    }
```

- [ ] **Step 5: Run the new test and the full suite**

Run: `cargo test --test integration_test non_ascii_layer_names 2>&1 | tail -3 && cargo test 2>&1 | tail -4`
Expected: new test PASS; **zero regressions** in the byte-exact sample roundtrips (they exercise descriptor `TEXT` values, slice names, and alpha channel names through the same helpers). If a byte-exact test regresses on trailing-NUL handling of a descriptor `TEXT`, the sample file will show exactly which side (strip-on-read vs append-on-write) is asymmetric — fix by making `write_unicode_string_with_padding` the strict inverse of the reader for that resource.

- [ ] **Step 6: Commit**

```bash
git add src/io/writer.rs src/io/reader.rs src/format/additional_info.rs tests/integration_test.rs
git commit -m "fix: correct UTF-16 length prefixes and surrogate pairs in unicode strings

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: PSB section length widths (PSB files currently unreadable)

**Files:**
- Modify: `src/io/reader.rs:204-241` (`read_section`) and its call sites (reader.rs lines ~430, 476, 495, 498, 607, 685, 1185, plus the test helper at ~1583)
- Test: `tests/integration_test.rs` (new test)

**Interfaces:**
- Produces: `pub fn read_section<F, T>(&mut self, round: usize, eight_byte: bool, func: F) -> Result<T>` — explicit per-call width flag replaces the implicit `self.large` check. `self.large` stays on the struct (still used for channel lengths and RLE counts).

- [ ] **Step 1: Write the failing test** (append to `tests/integration_test.rs`)

```rust
#[test]
fn psb_write_read_roundtrip() {
    use std::io::Cursor;
    let psd = Psd {
        width: 4,
        height: 4,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        image_data: Some(PixelData { data: vec![128u8; 4 * 4 * 4], width: 4, height: 4 }),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(4),
            right: Some(4),
            image_data: Some(PixelData { data: vec![200u8; 4 * 4 * 4], width: 4, height: 4 }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let bytes = write_psd(
        &psd,
        &WriteOptions { psb: Some(true), compress: Some(false), ..Default::default() },
    )
    .unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!((read.width, read.height), (4, 4));
    assert_eq!(read.children.as_ref().map(|c| c.len()), Some(1));
    let layer = &read.children.unwrap()[0];
    assert_eq!(layer.image_data.as_ref().unwrap().data[0], 200);
}
```

(`compress: Some(false)` isolates this task from the RLE bug fixed in Task 6.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test integration_test psb_write_read 2>&1 | tail -5`
Expected: FAIL with `Invalid resource signature` (reader consumes 8 bytes for the color-mode-data length that the writer stored in 4).

- [ ] **Step 3: Make the width explicit in `read_section`**

In `src/io/reader.rs:204`, change the signature and length read:

```rust
    /// Read a section with length prefix. `eight_byte` selects the PSB
    /// 8-byte length variant (layer-and-mask and layer-info sections only).
    pub fn read_section<F, T>(&mut self, round: usize, eight_byte: bool, func: F) -> Result<T>
    where
        F: FnOnce(&mut Self, u64) -> Result<T>,
    {
        let length = if eight_byte {
            let high = self.read_u32()? as usize;
            if high != 0 {
                return Err(PsdError::UnsupportedFeature(
                    "Sizes larger than 4GB are not supported".to_string(),
                ));
            }
            self.read_u32()? as usize
        } else {
            self.read_u32()? as usize
        };
        // ... rest unchanged ...
```

Update every call site (`grep -n "read_section(" src/`):

| Call site | `eight_byte` value |
|---|---|
| `read_color_mode_data` (reader.rs ~430) | `false` |
| `read_image_resources` (reader.rs ~476) | `false` |
| `read_layer_and_mask_info` outer (reader.rs ~495) | `reader.large` |
| layer-info inner section (reader.rs ~498) | `reader.large` |
| layer-record extra data (reader.rs ~607) | `false` |
| `read_layer_mask_data` (reader.rs ~685) | `false` |
| `read_global_layer_mask_info` (reader.rs ~1185) | `false` |
| test helper `read_flat_layers_for_sample` (reader.rs ~1583, two calls) | outer `reader.large`, inner `reader.large` |

Also check `grep -rn "\.read_section(" src/format/` — update any hits there with `false` unless they read the layer-info section.

- [ ] **Step 4: Run the new test and the full suite**

Run: `cargo test --test integration_test psb_write_read 2>&1 | tail -3 && cargo test 2>&1 | tail -4`
Expected: PASS, no regressions (all existing samples are PSD v1, where both paths read 4 bytes).

- [ ] **Step 5: Commit**

```bash
git add src/io/reader.rs src/format/ tests/integration_test.rs
git commit -m "fix: PSB uses 4-byte lengths for all sections except layer-and-mask info

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: PSB RLE row byte-counts (write 4-byte counts, stop read truncation)

**Files:**
- Modify: `src/support/compression.rs:15-119` (`decompress_rle` signature, `compress_rle` split), `src/io/reader.rs` (RLE count reads at ~831, ~1025, ~1270), `src/io/writer.rs` (`write_image_data` RLE branch ~840-858, `layer_channel_payload` ~937, `prepare_layer_channels` ~965)
- Test: `src/support/compression.rs` unit tests, `tests/integration_test.rs`

**Interfaces:**
- Produces:
  - `pub fn compress_rle_rows(data: &[u8], row_bytes: usize, height: usize) -> Result<(Vec<u32>, Vec<u8>)>` — returns (per-row byte counts, concatenated compressed rows); callers emit the count table at the width they need.
  - `pub fn compress_rle(data: &[u8], row_bytes: usize, height: usize, large: bool) -> Result<Vec<u8>>` — convenience wrapper emitting a 2-byte (PSD) or 4-byte (PSB) count table followed by the rows.
  - `pub fn decompress_rle(input: &[u8], output: &mut [u8], width: usize, height: usize, byte_counts: &[u32]) -> Result<()>` — counts widened from `&[u16]` to `&[u32]`.

- [ ] **Step 1: Write the failing tests**

In `src/support/compression.rs` tests:

```rust
#[test]
fn compress_rle_large_emits_four_byte_counts() {
    let data = vec![7u8; 16];
    let psb = compress_rle(&data, 16, 1, true).unwrap();
    let count = u32::from_be_bytes(psb[0..4].try_into().unwrap()) as usize;
    let mut out = vec![0u8; 16];
    decompress_rle(&psb[4..], &mut out, 16, 1, &[count as u32]).unwrap();
    assert_eq!(out, data);

    let psd = compress_rle(&data, 16, 1, false).unwrap();
    assert_eq!(psd.len(), psb.len() - 2); // 2- vs 4-byte table for one row
}
```

In `tests/integration_test.rs`:

```rust
#[test]
fn psb_rle_roundtrip() {
    use std::io::Cursor;
    let psd = Psd {
        width: 8,
        height: 8,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        image_data: Some(PixelData { data: vec![99u8; 8 * 8 * 4], width: 8, height: 8 }),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(8),
            right: Some(8),
            image_data: Some(PixelData { data: vec![50u8; 8 * 8 * 4], width: 8, height: 8 }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    // Default options => 8-bit => RLE compression.
    let bytes = write_psd(&psd, &WriteOptions { psb: Some(true), ..Default::default() }).unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!(read.children.unwrap()[0].image_data.as_ref().unwrap().data[0], 50);
    assert_eq!(read.image_data.unwrap().data[0], 99);
}
```

- [ ] **Step 2: Run to verify failures**

Run: `cargo test compress_rle_large 2>&1 | tail -3` — compile error (no `large` param) counts as the failing state.
Run: `cargo test --test integration_test psb_rle_roundtrip 2>&1 | tail -3` — Expected: FAIL (2-byte tables written, 4-byte tables read).

- [ ] **Step 3: Implement in `compression.rs`**

```rust
/// Compress rows with RLE; returns (per-row byte counts, concatenated rows).
pub fn compress_rle_rows(data: &[u8], row_bytes: usize, height: usize) -> Result<(Vec<u32>, Vec<u8>)> {
    let mut rows = Vec::new();
    let mut counts = Vec::with_capacity(height);
    for y in 0..height {
        let row = &data[y * row_bytes..(y + 1) * row_bytes];
        let compressed = compress_rle_row(row)?;
        counts.push(compressed.len() as u32);
        rows.extend_from_slice(&compressed);
    }
    Ok((counts, rows))
}

/// Compress with the row-count table prepended (2-byte PSD / 4-byte PSB counts).
pub fn compress_rle(data: &[u8], row_bytes: usize, height: usize, large: bool) -> Result<Vec<u8>> {
    let (counts, rows) = compress_rle_rows(data, row_bytes, height)?;
    let entry = if large { 4 } else { 2 };
    let mut output = Vec::with_capacity(counts.len() * entry + rows.len());
    for count in counts {
        if large {
            output.extend_from_slice(&count.to_be_bytes());
        } else {
            output.extend_from_slice(&(count as u16).to_be_bytes());
        }
    }
    output.extend_from_slice(&rows);
    Ok(output)
}
```

Widen `decompress_rle`'s `byte_counts: &[u16]` to `&[u32]` (body already converts to `usize`). Update the existing `test_compress_decompress_rle` for the new signatures.

- [ ] **Step 4: Update reader call sites**

At `src/io/reader.rs` ~831, ~1025, ~1270, replace the truncating reads:

```rust
let v: u32 = if reader.large {
    reader.read_u32()?
} else {
    reader.read_u16()? as u32
};
byte_counts.push(v);
```

with `byte_counts: Vec<u32>`, and where compressed lengths are summed (`reader.rs:1281`) sum `u32 as usize`.

- [ ] **Step 5: Update writer call sites**

`psb` is already in scope (from `options`) in each:
- `write_image_data` RLE branch (writer.rs ~840): use `compress_rle_rows`, then write all count tables first (as u16 or u32 per `psb`), then all row data. Replace the hardcoded `let table_len = height * 2;` splitting logic entirely.
- `layer_channel_payload` (~937) and the `raw_data` path in `prepare_layer_channels` (~965): pass `psb` to `compress_rle(..., psb)`. Both functions need the flag — thread `options.psb.unwrap_or(false)` through (both already receive `options` or are called where it's available; `prepare_layer_channels` raw path should honor `raw_data.large` when re-emitting preserved channels, falling back to `options.psb`).

- [ ] **Step 6: Run new tests + full suite**

Run: `cargo test compress_rle 2>&1 | tail -3 && cargo test --test integration_test psb_rle 2>&1 | tail -3 && cargo test 2>&1 | tail -4`
Expected: all PASS, no regressions (PSD paths still write 2-byte tables).

- [ ] **Step 7: Commit**

```bash
git add src/support/compression.rs src/io/reader.rs src/io/writer.rs tests/integration_test.rs
git commit -m "fix: 4-byte RLE row counts for PSB on both read and write

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: Guard non-RGB synthesized layer writes, fix README claims

**Files:**
- Modify: `src/io/writer.rs` (`write_layer_info` ~477 or `prepare_layer_channels` ~951), `README.md` (Current Status table)
- Test: `tests/integration_test.rs`

**Interfaces:**
- Produces: writing a non-RGB document whose layers lack `raw_data` returns `PsdError::UnsupportedFeature`. Documents whose layers all carry `raw_data` (i.e. anything read by this crate with default options) are unaffected.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn writing_synthesized_layers_in_non_rgb_mode_errors() {
    let psd = Psd {
        width: 1,
        height: 1,
        color_mode: Some(ColorMode::CMYK),
        bits_per_channel: Some(8),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData { data: vec![1, 2, 3, 4], width: 1, height: 1 }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let err = write_psd(&psd, &WriteOptions::default()).unwrap_err();
    assert!(matches!(err, psd_great::PsdError::UnsupportedFeature(_)));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test integration_test writing_synthesized_layers_in_non_rgb 2>&1 | tail -3`
Expected: FAIL — currently writes RGB channel IDs into a CMYK document without error.

- [ ] **Step 3: Implement the guard**

In `write_layer_info` (writer.rs ~486), before preparing payloads:

```rust
        let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
        if color_mode != ColorMode::RGB {
            // ponytail: synthesized layer channels are RGB-only; non-RGB layer
            // pixels roundtrip via raw_data. Full CMYK/Grayscale channel
            // synthesis is the upgrade path if ever needed.
            let has_synthesized = layers
                .iter()
                .any(|layer| layer.raw_data.is_none() && layer.image_data.is_some());
            if has_synthesized {
                return Err(PsdError::UnsupportedFeature(format!(
                    "Writing layers without raw channel data is only supported in RGB mode (document is {color_mode:?})"
                )));
            }
        }
```

(Place it after `let layers = flatten_layers(...)`. Import `PsdError` is already in scope.)

- [ ] **Step 4: Update README**

In the Current Status table, change the Color modes row to state explicitly: reading converts composites for RGB/Grayscale/CMYK/Indexed/Bitmap; **CMYK/Grayscale layer pixel channels are preserved via raw data only** — synthesizing new non-RGB layers from RGBA `image_data` is unsupported and returns an error.

- [ ] **Step 5: Run the test + full suite, commit**

Run: `cargo test 2>&1 | tail -4`
Expected: all PASS (sample corpus is read→write, so every layer has `raw_data`).

```bash
git add src/io/writer.rs README.md tests/integration_test.rs
git commit -m "fix: reject synthesized layer channels for non-RGB documents

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Hardening — writer capacity loop, layer-count overflow

**Files:**
- Modify: `src/io/writer.rs:53-62` (`ensure_capacity`), `src/io/writer.rs:485-497` (`write_layer_info`), `src/io/writer.rs:759-779` (`write_nested_layer_info_block`)
- Test: `src/io/writer.rs` unit tests

- [ ] **Step 1: Write the failing tests** (in `src/io/writer.rs` `mod tests`)

```rust
#[test]
fn zero_capacity_writer_does_not_hang() {
    let mut writer = PsdWriter::new(0);
    writer.write_u8(42).unwrap();
    assert_eq!(writer.get_buffer(), &[42]);
}

#[test]
fn layer_count_over_i16_max_errors() {
    assert!(layer_count_i16(40_000).is_err());
    assert_eq!(layer_count_i16(3).unwrap(), 3);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib io::writer::tests::layer_count_over 2>&1 | tail -3` — compile error (no `layer_count_i16`).
Do **not** run `zero_capacity_writer_does_not_hang` yet — it currently loops forever. Verify by reading `ensure_capacity`: `new_capacity *= 2` from a starting capacity of 0 never reaches `required`.

- [ ] **Step 3: Implement**

```rust
    /// Ensure buffer has enough capacity
    fn ensure_capacity(&mut self, additional: usize) {
        let required = self.offset + additional;
        if self.buffer.len() < required {
            // Doubling from a floor of 64 avoids the infinite loop when
            // the writer was created with capacity 0.
            let new_len = required.max(self.buffer.capacity().max(64) * 2);
            self.buffer.resize(new_len, 0);
        }
    }
```

Add near `flatten_layers`:

```rust
fn layer_count_i16(count: usize) -> Result<i16> {
    i16::try_from(count).map_err(|_| {
        PsdError::InvalidFormat(format!("Too many layers: {count} (max {})", i16::MAX))
    })
}
```

Use it in `write_layer_info` (replacing `layers.len() as i16`, negating after conversion for the global-alpha case) and in `write_nested_layer_info_block` (replacing `flattened.len() as i16`).

- [ ] **Step 4: Run both tests + full suite**

Run: `cargo test --lib io::writer::tests 2>&1 | tail -4 && cargo test 2>&1 | tail -4`
Expected: PASS, zero-capacity test completes instantly.

- [ ] **Step 5: Commit**

```bash
git add src/io/writer.rs
git commit -m "fix: writer capacity growth from zero and layer-count overflow guard

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Spec-sweep follow-ups (from 2026-07-03 spec conformance check)

**Files:**
- Modify: `src/format/image_resources.rs:1345-1352` (resource read catch-up), `src/format/additional_info.rs` TySh bounds fields (~1703 read, ~2589 write, `TextLayerData` struct ~440)
- Test: `src/format/image_resources.rs` unit tests

**Interfaces:**
- Produces: `TextLayerData` bounds fields become `pub left: i32, pub top: i32, pub right: i32, pub bottom: i32` (spec says 4×f64 but real files use 4×4 bytes; upstream ag-psd reads int32).

- [ ] **Step 1: Resource over-read guard.** In `read_image_resources` (image_resources.rs:1345), replace the forward-only catch-up with a deterministic seek so a handler that over-reads cannot desync all following resources:

```rust
        reader.seek_to(resource_start + data_length as u64)?;
        if data_length % 2 != 0 {
            reader.skip_bytes(1)?;
        }
```

(Deletes the `consumed`/`if data_length > consumed` block.) Add a unit test: a resource whose handler under-reads (e.g. unknown id with trailing junk) followed by a second resource; assert the second one is parsed.

- [ ] **Step 2: TySh bounds as i32.** Change the four bounds fields on `TextLayerData` from `f32` to `i32`; read with `read_i32`, write with `write_i32`. Byte width is unchanged (16 bytes), so raw roundtrips are unaffected; only the semantic values become meaningful. Update any field initializers (`0.0` → `0`).

- [ ] **Step 3: Run full suite, commit**

```bash
git add src/format/image_resources.rs src/format/additional_info.rs
git commit -m "fix: deterministic resource block advance; TySh bounds are int32

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo test 2>&1 | tail -6` — everything green (one pre-existing `#[ignore]` allowed; re-evaluate whether its ignore message is still accurate after Task 1 and un-ignore it if the TS parser now accepts the output).
- [ ] Optional interop smoke: `cargo run --example resave_psd` against a sample, then parse the output with the TS parser (see the ignored test `test_roundtrip_sample_opens_in_ts_parser_subprocess` for the exact `npx tsx` invocation).
- [ ] `git log --oneline master..HEAD` — one commit per task, each self-contained.
