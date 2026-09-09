# PSD/PSB parser and writer audit

Date: 2026-09-07. Reviewed commit: `40b449d167449b9adafbeb39c75f308a7f20288b`.

This is a suggested-change report only. No implementation, tests, dependency files, or existing plans were edited. Existing untracked files and local changes were left alone. The audit covered the public model, PSD reader/writer, compression, descriptors, tagged blocks, resources, text preprocessing, adjacent formats, and test strategy. Review depth was greatest in the shared binary and pixel paths; this is not a claim that every feature-specific handler is correct or that every possible bug has been found.

The current implementation is not safe to describe as lossless or fully interoperable. There are reproducible data-loss and malformed-output cases despite a passing test suite. Prioritize correct wire encoding, preservation, and bounded parsing before adding more typed features.

## Validation and evidence

- `cargo test --all-targets --locked` completed successfully. The byte-exact sample roundtrip test remains explicitly ignored. Passing Rust self-roundtrips do not establish Photoshop compatibility.
- Two temporary Rust executables, compiled against the current crate outside the repository, exercised public APIs. Their relevant results are recorded below. They did not modify source files or sample PSDs.
- Inspected the eight in-repository PSD fixtures and the corpus test definitions. Several tests also depend on an external sibling checkout.
- Compared disputed encodings with [Adobe's file-format specification](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/) and primary source from the independent psd-tools implementation. No Photoshop UI opening/rendering was performed.
- No sustained fuzzing, multi-gigabyte allocation experiments, exhaustive feature corpus, or performance benchmark was run. Findings below explicitly distinguish execution evidence from static inspection.

Severity: **P1** means corruption, lost document content, invalid interoperability, or a parser robustness failure; **P2** means a narrower correctness/API/support gap; **P3** means maintainability or validation improvements. Findings can share a repair and should not be counted as independent estimates of implementation effort.

## Parser, compression, and size safety

### 01 — P1: ZIP streams use the wrong wrapper

**Location:** `src/support/compression.rs:172`, `:184`; all layer/composite ZIP callers.

The helpers use `DeflateDecoder`/`DeflateEncoder`, explicitly excluding zlib framing. The unit test `zip_roundtrip_is_raw_deflate_not_zlib` entrenches that choice. Independent PSD ZIP handling uses zlib streams. See [psd-tools compression implementation](https://psd-tools.readthedocs.io/en/stable/_modules/psd_tools/compression.html).

**Evidence:** `compress_zip(&[1,2,3])` produced `[99,100,98,6,0]`, a raw DEFLATE stream. Matching local decoding proves only internal consistency.

**Suggested change:** use the existing flate2 zlib encoder/decoder for PSD ZIP. If existing crate-generated raw-DEFLATE files need recovery, make that an explicit compatibility read path, not the standard writer format.

**Regression gate:** decode independently generated ZIP layers and composites; independently decode crate output at 8/16/32-bit depth. Replace the test that demands raw DEFLATE.

### 02 — P1: 16/32-bit prediction disagrees with independent decoding

**Location:** `src/support/compression.rs:199`, `:271`; `src/io/reader.rs:1366`; `src/io/writer.rs:1139`.

The 16-bit paths difference individual bytes. The 32-bit paths shuffle bytes but restart deltas at each byte-plane boundary. Independent decoding instead uses 16-bit sample deltas and, for 32-bit, a delta across the entire shuffled row. See [psd-tools prediction functions](https://psd-tools.readthedocs.io/en/stable/_modules/psd_tools/compression.html#decode_prediction).

**Suggested change:** implement the correct per-depth transform once and share it between layer and composite code. Fix framing from finding 01 at the same time.

**Regression gate:** external expected bytes with 16-bit carries/borrows and 32-bit nonzero plane transitions, multiple rows, and more than one channel. Existing symmetric roundtrips cannot catch this.

### 03 — P1: decompression size is neither enforced nor bounded

**Location:** `src/support/compression.rs:172`, `:199`; `src/io/reader.rs:1476`.

`output_size` is only a capacity hint; `read_to_end` grows without a limit. Prediction indexes the returned buffer without checking its size. Other callers silently pad/truncate via `normalize_channel_data`.

**Executed evidence:** a compressed three-byte payload decoded successfully with expected size one and returned all three bytes. Prediction of a valid compressed empty stream at width two panicked inside `catch_unwind`.

**Suggested change:** cap output at the validated expected length plus one, reject short/long decoded output, check stream completion, and remove silent repair from strict decoding. Provide explicit recovery diagnostics if tolerant decoding is retained.

**Regression gate:** valid-but-short streams, oversized streams, truncated compressed streams, incorrect checksums, and a small compressed expansion bomb must return errors without panic or unbounded growth.

### 04 — P1: RLE does not enforce scanline boundaries or decoded row length

**Location:** `src/support/compression.rs:15`.

The width argument is unused. Output position spans rows; literal/repeat reads check the whole input instead of `row_end`. Underfilled rows succeed, and unused bytes are skipped when output fills early.

**Executed evidence:** decoding `[0,7]` into a two-byte row returns `Ok(())` and `[7,0]`. A literal declaring two bytes can consume bytes belonging to the next row before a later error is noticed.

**Suggested change:** decode each bounded row into exactly its destination row slice. Validate every run against both row bounds and require exactly the expected decoded size, allowing only valid no-op packets.

**Regression gate:** empty/short/overlong rows, runs crossing either boundary, no-op packets, missing counts, and valid maximum PackBits runs.

### 05 — P1: section lengths do not constrain reads or seeks

**Location:** `src/io/reader.rs:113`, `:126`, `:205`; `src/format/additional_info.rs` `read_layer_additional_info`; `src/format/image_resources.rs:1227`.

`read_section` passes an end offset but does not prevent a handler reading beyond it or reject overconsumption afterward. Seeking to the declared end can pass EOF. Resource/tagged-block handlers operate on the parent reader and then seek back/forward to the advertised boundary, masking overreads. `bytes_left` converts overruns into zero.

**Executed evidence:** a one-byte section whose handler reads a u16 returned `Ok(1800)`, consuming the next byte. A section claiming 100 bytes with no payload returned success when its callback read nothing.

**Suggested change:** validate child length against parent bounds and physical input length, and parse through a bounded view/sub-reader. Use checked offset arithmetic and reject overreads before they consume sibling data. Do not silently reinterpret malformed blending lengths as layer names.

**Regression gate:** truncated headers/padding, child length exceeding parent, declared payload beyond EOF, handler overread, and nested under/overconsumption across all section types.

### 06 — P1: attacker-controlled allocations and arithmetic lack a shared budget

**Location:** `PsdReader::read_bytes`, Unicode/descriptor count readers, `src/io/reader.rs:766`, `:984`, `:1118`; ABR/CSH readers.

Lengths and counts feed `vec!`/`Vec::with_capacity` before validation against available bytes. Layer bounds use signed subtraction before conversion; hostile `i32` extremes can overflow. Header dimension maxima still permit enormous allocations. Descriptor recursion and group/text structures have no depth/node budget.

**Suggested change:** introduce a small shared read-limits policy for total allocated bytes, decoded pixels, nodes, and nesting depth. Check counts against enclosing bytes before allocating; use checked multiplication/addition and wider signed arithmetic for rectangles. Use fallible reserve for large allocations. Apply limits to adjacent formats too.

**Regression gate:** tiny inputs with huge counts, extreme rectangles, excessive nested descriptors, and oversized-but-format-legal headers return structured errors. Test debug and release behavior; do not run OOM probes in ordinary CI.

### 07 — P1: PSB tagged-block length width is inferred only from signature

**Location:** `src/format/additional_info.rs` `read_layer_additional_info`, `tagged_block_uses_u64_length`, `write_tagged_block`.

The reader consumes eight bytes only for signature `8B64`. The writer knows the PSB key list, but that knowledge is not used when reading an `8BIM` block in a PSB. Adobe specifies length width by PSB key as well as allowing both signatures. See [additional layer information](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/).

**Suggested change:** derive length width from document version and key, with explicitly tested compatibility for signature variants. Keep the key table shared by both directions.

**Regression gate:** hand-built PSB `8BIM` blocks with eight-byte lengths, particularly `Lr16`, `Lr32`, and `lnk2`, followed by another block. A crate-generated `8B64` roundtrip is insufficient.

### 08 — P1: nested high-depth layer information loses PSB context

**Location:** `src/io/reader.rs` `read_nested_layer_info_block`; `src/io/writer.rs` `write_nested_layer_info_block`; `additional_info.rs` `Lr16`/`Lr32` dispatch.

Nested reading creates a default reader with `large=false`; nested writing creates default PSD options. Color mode is also replaced with RGB in nested raw data, and negative layer-count transparency information is discarded.

**Suggested change:** pass document version, color mode, depth, options, and relevant alpha state into nested layer handling. Decide how `Lr16`/`Lr32` becomes the active editable layer tree instead of leaving it disconnected in `additional_info.high_depth_layer_data`.

**Regression gate:** externally authored PSB high-depth layer blocks, nested groups/masks, and editing the effective tree before resaving.

### 09 — P1: changing PSD/PSB output format retains the input RLE count width

**Location:** `src/io/writer.rs:969`, especially the `raw.large` argument to `compress_rle`.

Raw-channel RLE chooses its count width from `LayerRawData.large`, while layer headers use `WriteOptions.psb`. Converting preserved raw data between versions therefore creates conflicting record layouts.

**Suggested change:** use the destination version for every newly encoded row-count table, regardless of source version. Keep source version only for decoding source bytes.

**Regression gate:** explicitly supplied/preserved RLE raw channels written PSD→PSB and PSB→PSD, inspected by an independent decoder.

### 10 — P1: large lengths and RLE counts silently narrow

**Location:** `PsdWriter::write_section_with_length_mode`; `src/io/writer.rs:588`; `src/support/compression.rs:116`; `write_tagged_block`; ASE string/count writers.

Section and channel lengths are cast to u32; PSB high words are always zero. PSD RLE lengths are cast to u16. Public compression helpers also slice dimensions without validating the input length. Readers reject sizes over 4 GB while the writer may silently wrap them.

**Suggested change:** use checked conversions for every wire count. Either implement true u64 PSB lengths or explicitly reject oversized values before writing. Reject oversized PSD RLE rows or select another supported compression. Validate helper dimensions before slicing.

**Regression gate:** boundary checks around u16/u32 maxima through small factored length-validation tests, plus malformed helper inputs. No multi-gigabyte fixture is necessary to test conversion logic.

## Pixels, layers, masks, and document structure

### 11 — P1: folder reconstruction discards the actual folder record

**Location:** `src/io/reader.rs:1069`; `src/io/writer.rs` `flatten_layers`.

On an open/closed folder marker, the reader retains only its name and open state. It uses the bounding-divider record as the public group. Folder opacity, visibility, blend mode, effects, masks, text/metadata, and other properties are therefore discarded. Empty groups get `children=None`, so writing stops recognizing them as groups. Unbalanced markers are also accepted without a structural error.

**Executed evidence:** a hidden Multiply group with opacity 0.25 roundtrips as visible Normal with opacity 1.0. An empty group's `Some(vec![])` becomes `None`.

**Suggested change:** keep the complete folder record on the stack and attach children to it at the closing boundary. Preserve `Some(empty)` for empty groups and retain separator metadata separately only when needed. Validate balanced markers. Avoid duplicating the group's layer ID onto a synthetic separator without evidence that this is required.

**Regression gate:** nested and empty groups with distinct IDs, opacity, hidden state, effects, blend mode, and masks; malformed nesting; semantic comparison against the original wire records.

### 12 — P1: clipping is discarded on read and forced off on write

**Location:** `src/io/reader.rs:585`; `src/io/writer.rs:643`; `src/api/layer.rs:699`.

The reader never assigns `blend.clipping`; the writer emits zero. The comment substituting resource 1026 is not supported by the model or wire semantics: that resource contains dragging-group IDs, while the layer record has its own clipping byte. See [Adobe layer records and resource IDs](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/).

**Executed evidence:** a layer with `clipping=Some(1)` reads back as `None`.

**Suggested change:** preserve and serialize the layer clipping byte; validate supported values without conflating it with group IDs.

**Regression gate:** a clipped layer over a base layer, including clipped layers inside folders, opened by an independent application.

### 13 — P1: ordinary layer decoding treats high-depth bytes as 8-bit pixels

**Location:** `src/io/reader.rs:766`; `:984`; `ReadOptions::use_raw_data`.

The main layer path computes `width*height`, reads/normalizes to that byte count, and assigns bytes directly to RGBA. It does not account for 16/32-bit samples. The separate raw reader is only used for nested high-depth blocks; the public `use_raw_data` flag is never consulted.

**Executed evidence:** two different 16-bit-written source pixels `[40,80,120,255]`, `[200,210,220,255]` read back as two copies of the first pixel. Even with `use_raw_data=true`, `raw_data` is `None`.

**Suggested change:** route layer decoding through a common native-sample channel decoder, retain original depth, and generate an optional RGBA preview separately.

**Regression gate:** independent 16/32-bit layer channels under raw/RLE/ZIP, multiple distinct pixels per row, HDR values, and lower-bit precision preservation.

### 14 — P1: non-RGB layer preservation does not match the advertised behavior

**Location:** `src/io/reader.rs:766`, `channel_offset`; `src/io/writer.rs:471`; `README.md` color-mode status.

Ordinary CMYK/Grayscale layers take the RGBA decoding path and do not acquire `raw_data`. CMYK's fourth component is routed into alpha, and its actual transparency is not handled by that RGBA switch. Default-loaded non-RGB layers then fail the writer's raw-data guard. The guard also permits raw-data objects whose depth does not match output, even though channel preparation then falls back to RGB synthesis.

**Suggested change:** preserve native channels for every non-RGB mode; derive previews only through mode-specific conversion. Validate raw color mode/depth compatibility before choosing a synthesis path. Reject unsupported synthesis explicitly.

**Regression gate:** default-read/default-write CMYK and Grayscale layers; CMYK transparency; depth changes with raw data present; no silent color-channel reinterpretation.

### 15 — P1: composite writing always emits RGB(A) planes

**Location:** `src/io/writer.rs:354`, `:819`.

The header computes a mode-dependent channel count, but image writing always uses offsets `[0,1,2]` or `[0,1,2,3]`. This is wrong for Grayscale, Indexed, CMYK, Duotone, Multichannel, and Lab. Some outputs have extra bytes; others are short or have misaligned RLE tables.

**Executed evidence:** a one-pixel CMYK document successfully writes but fails to read with `UnexpectedEof`.

**Suggested change:** share one validated channel plan between header and composite emission. Until native serialization exists, reject unsupported color modes rather than writing successful-looking invalid data. Do not infer Indexed indices from an RGB component.

**Regression gate:** mode-by-mode header/channel payload agreement under raw and compressed output, including palette lookup and alpha.

### 16 — P1: Bitmap and Indexed composite support is incomplete in executable code

**Location:** `src/io/reader.rs:1203`; `src/io/writer.rs:346`; `README.md`.

Bitmap is allowed by the mode check, but valid depth 1 is then rejected. Indexed bytes are assigned to the red channel without palette expansion. The generic base-channel fallback treats both modes as three-channel when deriving defaults.

**Suggested change:** implement packed Bitmap rows with correct row rounding and polarity, and Indexed palette lookup/transparency. Either support depth-1 writing or explicitly document a read-only limitation; remove the current claim that Bitmap composites are decoded until implemented.

**Regression gate:** odd-width Bitmap images and non-grayscale palettes with nontrivial indices; independently authored files.

### 17 — P1: composite alpha and auxiliary channels are not preserved

**Location:** `src/io/reader.rs:1231`, `:1313`; `src/io/writer.rs:354`; `Psd.channels` and `Psd.image_data`.

The Grayscale path reads a second channel into alpha and then overwrites it with 255 because `total_channels <= 3`. RGB's fourth channel is treated as transparency regardless of the global-alpha marker, potentially confusing saved alpha channels with merged transparency. Additional planes are discarded. The writer ignores `Psd.channels` and derives output solely from base color mode plus nonopaque RGBA bytes.

**Executed evidence:** the Grayscale probe with alpha 128 returns alpha 255. This is corroborated by the read-side assignment itself, independently of finding 15.

**Suggested change:** retain explicit merged-transparency state and all extra channel planes/identities. Apply default opaque alpha relative to mode base-channel count, not a fixed three. Handle opaque-but-present transparency separately from absent transparency.

**Regression gate:** grayscale+alpha, RGB saved alpha without merged transparency, opaque merged transparency, multiple saved alpha/spot channels, and channel-name/ID alignment.

### 18 — P1: composite precision is irreversibly reduced

**Location:** `src/io/reader.rs:1428`; `src/io/writer.rs:1201`; `src/api/psd.rs:304`.

The only composite storage is byte RGBA. 16-bit samples discard lower bits; 32-bit samples are clamped and quantized to 8-bit. Writing expands those quantized values back to 16/32-bit. CMYK composite conversion additionally accesses `plane[i]` directly, bypassing depth-aware sample conversion.

**Suggested change:** preserve native composite channels, depth, and color mode; treat byte RGBA as an optional preview. Make conversion explicit and document rendering/color-management limits. Never substitute the preview for native samples on an unchanged save.

**Regression gate:** non-repeated 16-bit bytes, distinct floating-point values mapping to the same 8-bit value, values above 1.0/below 0.0, and high-depth CMYK sample indexing.

### 19 — P1: masks are baked into layer alpha using incorrect semantics

**Location:** `src/io/reader.rs:895` onward.

The parser applies `min(alpha, mask)` rather than keeping the underlying layer alpha separate. It ignores the disabled flag, outside-mask default color, relative positioning flag, density, and feather. Alpha 128 with mask 64 becomes 64 instead of the expected multiplicative coverage near 32 if producing a rendered preview. Writing the parsed pixels together with the preserved mask can apply the mask again.

**Executed evidence:** a disabled mask still changes source alpha 128 to 64.

**Suggested change:** keep unmasked layer pixels and mask channels independent. If preview rendering is offered, put it in an explicit helper with defined mask behavior. Do not make parsing implicitly destructive.

**Regression gate:** partial alpha plus partial mask, disabled masks, nonwhite outside coverage, relative coordinates, user/real masks, and repeated read/write preservation of original channel bytes.

### 20 — P1: mask metadata layout uses heuristic and synthetic fields

**Location:** `src/io/reader.rs:675`; `src/io/writer.rs:662` onward.

The reader only examines parameter flags when at least 18 bytes remain, skipping compact valid parameter payloads. It ties real-mask metadata to channel -3 and sometimes excludes it when -3 exists. The writer emits real-mask fields before parameters only under `has_params`, loses them otherwise, and appends a fixed 40-byte `0x0006` filler to every mask. The standard orders parameters before the optional real-mask structure. See [Adobe mask structure](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/).

**Suggested change:** parse and emit the documented layouts based on bounded length and flags; preserve any demonstrable vendor extension separately. Do not globally inject a sample-specific filler.

**Regression gate:** 20-byte basic mask; minimal density-only parameters; feather-only parameters; real mask with and without parameters and -3 channel; independent Photoshop samples.

### 21 — P1: blending-range public model represents half a range

**Location:** `src/io/reader.rs:1487`; `src/io/writer.rs:1277`; `LayerBlendingRangePair`.

The parser treats each four bytes as a complete source/destination pair. The writer reproduces that representation, while its default generator emits eight bytes per actual pair. The public fields cannot express both split black/white endpoints for source and destination. Adobe specifies eight bytes per pair. See [blending ranges](https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/).

**Suggested change:** model all eight values per pair, validate payload multiples, and serialize composite plus channel pairs in the correct order.

**Regression gate:** distinct values in all eight positions, editing one destination endpoint, and an independent Blend If fixture. Byte-preserving an incorrectly named four-byte grouping is not semantic correctness.

### 22 — P1: pixel dimensions and buffers are not validated before writing

**Location:** `src/io/writer.rs:819`, `:1223`, `:1245`, `:1259`.

The composite payload takes dimensions from `PixelData`, while the header takes `Psd.width/height`. Layer extraction uses rectangle dimensions but ignores pixel-data dimensions and silently pads/truncates missing bytes. Raw channels can also have inconsistent sizes.

**Executed evidence:** document width two with a one-pixel composite writes successfully, then fails to read with EOF.

**Suggested change:** validate document/composite agreement, layer/mask rectangle agreement, checked exact buffer lengths, and finite/range-constrained numeric fields at the write boundary. If cropping or padding is desired, expose it as an explicit operation.

**Regression gate:** mismatched dimensions, short/long RGBA and mask buffers, resized layers with old raw channels, and invalid rectangles must fail predictably.

### 23 — P2: skipping pixels followed by writing silently fabricates replacement content

**Location:** `src/io/reader.rs:409`, `:766`; `src/io/writer.rs:819`, `:1223`.

Skipping composite/layer image data leaves no preserved payload or distinct “unloaded” state. The writer treats it like a newly created document/layer with no image and synthesizes black/opaque data. The parity suite even performs a second write after skipping the composite, without checking its pixels.

**Suggested change:** distinguish omitted-for-new-document from skipped-on-read. Preserve compressed spans where feasible, or refuse to overwrite unloaded content unless explicitly requested.

**Regression gate:** metadata-only read/edit/save preserves imagery or produces a clear error, never a successful silent replacement.

### 24 — P2: original layer flags and zero-sized-layer channels are lost

**Location:** `src/io/reader.rs:590`, `:778`; `src/io/writer.rs:634`.

Only selected blend flags survive; writing rebuilds flags from defaults. Pixel-data-irrelevant and unmodeled bits are not preserved. Zero-width/height layers skip every channel, including masks that can have their own nonempty bounds.

**Suggested change:** retain original flag bits alongside typed accessors and preserve channel records independently of the layer color rectangle. Validate mask dimensions using their own bounds.

**Regression gate:** adjustment/vector/group records with meaningful flags and empty color bounds but nonempty mask channels.

## Descriptors, text, and resources

### 25 — P1: ordinary descriptor enums are rewritten as enumerated references

**Location:** `src/support/descriptor.rs:275`, `:525`, `:678`.

`enum` reads into `DescriptorValue::Enum`, but `ostype_sig` writes that value as `Enmr`, and its serializer adds a class structure. These are different wire types. This can affect text, effects, smart objects, and descriptor-backed adjustments broadly. Compare the distinct types in [psd-tools descriptor source](https://raw.githubusercontent.com/psd-tools/psd-tools/main/src/psd_tools/psd/descriptor.py).

**Executed evidence:** a descriptor enum `Blnd/Nrml` is emitted with `Enmr` and an extra class structure.

**Suggested change:** emit ordinary enums as `enum` with type/value IDs; reserve `Enmr` for an explicitly modeled reference item. Preserve original descriptor value kinds.

**Regression gate:** independent `enum` descriptor bytes, nested lists/descriptors containing enums, and external decode of newly created text/effects.

### 26 — P1: reference descriptors lose subtype fields and wire identity

**Location:** `src/support/descriptor.rs:230`, `:311`; `read_reference_structure`; `write_reference_structure`.

`obj ` and `VlLs` both become `List`; reference-specific parsing discards class/name/offset fields and collapses values into generic variants. A better `ReferenceItem` model already exists but is bypassed by `read_ostype`.

**Executed evidence:** a written `Reference([Offset { name: "x", class_id: "Lyr ", offset: 42 }])` reads as `List([Class { name: "x", class_id: "Lyr " }])`; 42 is lost.

**Suggested change:** use the dedicated reference parser for `obj ` and retain each original subtype and all fields. Check subtype layouts against independent fixtures rather than assuming a generic class prefix everywhere.

**Regression gate:** every ReferenceItem variant, actual reference bytes from an independent producer, and read/write equality of fields and type signatures.

### 27 — P2: unknown units, short IDs, and object-array metadata are normalized destructively

**Location:** `src/support/descriptor.rs:283`, `:458`, `:552`, `:586`.

Unknown unit codes survive reading as strings but writing substitutes Pixels. Short IDs are padded into four-character IDs unless in a hardcoded allowlist. Object arrays discard their initial value and class name, then write fixed replacements. Descriptor storage in HashMap also cannot preserve duplicate keys or original ordering where preservation is required.

**Executed evidence:** a UnitDouble with `units="#XYZ"` reads back as `units="Pixels"`.

**Suggested change:** preserve unknown four-byte unit codes, write actual length-prefixed short string IDs, and retain object-array header/name fields. Keep duplicate/order preservation scoped to wire structures that need it rather than using unrelated allowlists.

**Regression gate:** unknown unit, one-to-three-character arbitrary IDs, object-array nondefault metadata, and duplicate-key policy tests.

### 28 — P1: Unicode fixes did not reach descriptor paths and gradient names

**Location:** `src/support/descriptor.rs` `Pth ` read/write; `src/api/adjustments.rs:340`, `:392`, gradient-map serialization.

These helpers still iterate Unicode scalar values, cast to u16, or decode one code unit at a time. Astral characters are corrupted even though the main PSD Unicode helper was fixed. ASE/CSH readers additionally remove all NUL units rather than just a terminator, and ASE lengths narrow unchecked to u16.

**Executed evidence:** a descriptor FilePath containing `😀` roundtrips as U+F600.

**Suggested change:** reuse correct UTF-16 unit encoding/decoding, with field-specific terminator and byte-order rules. Preserve opaque path payloads when their undocumented internal layout cannot be validated. Validate representable lengths and avoid deleting interior NULs indiscriminately.

**Regression gate:** non-BMP gradient names and descriptor paths, surrogate pairs, interior/trailing NULs, malformed UTF-16, and ASE names at the length boundary.

### 29 — P1: retained raw bytes silently override semantic edits

**Location:** `src/format/additional_info.rs:2564` (`TySh`), `Txt2` writer; `src/io/writer.rs:969`.

Loaded TySh records always carry `raw_bytes`, and their writer emits those bytes without examining edited text fields. Txt2 similarly prefers `raw`. Explicit/nested `LayerRawData` takes precedence over edited `image_data`. No dirty-state or public mutation contract resolves these conflicts.

**Suggested change:** define one clear edit contract: mutation methods invalidate the affected raw cache, or compare semantic snapshots before reusing it. Preserve raw data only while its modeled representation is unchanged. Document low-level manual escape hatches, but do not make silent ignored edits the default API behavior.

**Regression gate:** load a real text layer, change text/transform/descriptor, write and reopen; modify native/raw-backed pixels and mask pixels; ensure edits survive and unrelated opaque fields remain intact.

### 30 — P2: document text synthesis misses nested layers and can leave stale document data

**Location:** `src/io/writer.rs:1340` `apply_text_prewrite`.

It visits root children only and returns immediately when any document text-engine block exists. New/edited nested text is excluded; an existing Txt2 prevents reconciliation after changes. Parse failures while extracting EngineData are silently ignored.

**Suggested change:** traverse the effective tree recursively and explicitly reconcile text-object/resource references when text changes. Preserve existing engine bytes for unchanged text; reject or report unsupported edits instead of generating incomplete document resources.

**Regression gate:** root and nested text, adding/removing/reordering text objects in a loaded document, mixed fonts/resources, and malformed EngineData with contextual diagnostics.

### 31 — P1: resource 1077 is modeled as unrelated unit fields

**Location:** `src/support/binrw_support.rs:7`; `src/format/image_resources.rs:203`; public DisplayInfo types.

The implementation uses a 28-byte structure with a u16 version and little-endian display-unit fields. Independent PSD parsing models a u32 version followed by alpha-channel display records carrying color, opacity, and mode. A one-record payload can be shorter than 28 bytes, so the current parser returns None and the resource disappears on save. Longer inputs are reinterpreted and regenerated as unrelated units. See [psd-tools DisplayInfo and AlphaChannel source](https://raw.githubusercontent.com/psd-tools/psd-tools/main/src/psd_tools/psd/image_resources.py).

**Suggested change:** replace this model using independent fixtures. Until implemented, preserve resource 1077 opaquely rather than declaring it handled and losing it.

**Regression gate:** one and multiple alpha-channel display entries from Photoshop, preserving colors/opacity/mode and the exact payload when unchanged.

### 32 — P1: modeled resources lose envelopes, unsupported variants, and trailing data

**Location:** `src/format/image_resources.rs:1227`, `:1354`; tagged-block model dispatch/write subset.

Known resource names are read but not retained. Repeated modeled resources collapse into one field/map entry despite order tracking. Some parsers return None or partial defaults on bad/unknown payloads, and no raw fallback is saved. Tagged-block signatures are regenerated from key/version; repeated modeled blocks can likewise collapse. Version fields are often read and discarded, including descriptor wrappers.

**Suggested change:** retain a complete envelope and original payload for unmodified or unsupported records, including name, signature, version, multiplicity, and residual bytes. Only replace a known payload when an explicit semantic edit is made. Make malformed input an error in strict mode and retain it with diagnostics in recovery mode.

**Regression gate:** named known resources (including paths), duplicate modeled IDs/keys, short/unknown-version thumbnail and display-info payloads, unknown suffix bytes, and supported/unsupported variants interleaved in original order.

### 33 — P1: resource convenience fields change unedited resolution metadata

**Location:** `src/format/document_resource_postprocess.rs:72`, `:192`.

Reading copies horizontal resolution into a scalar `psd.resolution`. Writing then replaces the original resource with equal horizontal/vertical values and fixed inch units. Unequal resolutions and original measurement units are lost without an edit. Similar mirrored fields do not have a defined deletion/precedence contract: setting a convenience field to None leaves old low-level data in place.

**Suggested change:** retain the complete resolution resource and change only explicitly edited values. Establish one authoritative representation or dirty-aware convenience accessors for mirrored resources; make clearing a field reliably remove the intended metadata.

**Regression gate:** unequal X/Y resolution, centimeter units, editing one axis, and clearing ICC/selected IDs/other mirrored resources after loading.

### 34 — P2: resource 1072 maps only root layers and cannot reliably clear false values

**Location:** `src/format/document_resource_postprocess.rs:23`, `:141`.

Both directions traverse only root `children`; write emits a replacement only if some root layer remains false. Turning the final false value true leaves the old resource untouched. Group separators and descendants change the correspondence between resource entries and the public tree.

**Suggested change:** map resource entries against the original/canonical flat layer order with group markers, then expose the appropriate values on tree nodes. Rebuild or remove the resource after edits even when all values become true.

**Regression gate:** nested groups, independent flat-index fixtures, changing the last false to true, insertion/deletion/reordering of layers.

### 35 — P1: unknown channel IDs silently become the red channel

**Location:** `src/api/types.rs:606`; `src/io/reader.rs:565`.

`ChannelID::from_i16` maps every unsupported ID to Color0. Multiple extra channels can therefore overwrite red in decoding and are serialized under a different ID in raw/nested data.

**Suggested change:** retain arbitrary signed IDs with a checked semantic accessor, or reject unsupported IDs explicitly while preserving their raw payload. Do not use a valid color-channel ID as an error fallback.

**Regression gate:** channel IDs 4 and above, unusual negative IDs, and multiple extras with distinct payloads.

## Adjacent formats, public promises, and engineering work

### 36 — P2: most public read/write options are no-ops

**Location:** `src/api/psd.rs:345`; all option consumers in `src`.

Source search found declarations but no handling for `use_raw_data`, `use_image_data`, `skip_thumbnail`, `skip_linked_files_data`, `use_raw_thumbnail`, `strict`, and several logging/missing-feature flags. Writer flags such as `generate_thumbnail`, `trim_image_data`, `invalidate_text_layers`, and `no_background` likewise have no effect.

**Suggested change:** implement options that are part of the intended contract; otherwise remove/deprecate them or return an explicit unsupported-option error. Document defaults and the effect on roundtrip preservation. Prioritize strict parsing and raw preservation over convenience generation.

**Regression gate:** each retained option has an observable behavioral test. In particular, requesting strict mode must change malformed-input handling rather than merely populating a struct field.

### 37 — P2: ABR parsing silently returns incomplete brushes

**Location:** `src/formats/abr.rs:125`, `:241`.

Legacy brush handling reads but ignores compression, derives allocations directly from signed dimensions cast to usize, and does not honor known-record `size` bounds. Version 1/2 share a parser without passing version context. Modern ABR deliberately skips `samp`, `patt`, and `phry`, returns empty sample/pattern collections, swallows descriptor errors, and can return Ok on truncation.

**Suggested change:** either implement supported versions with bounded records, correct sample decompression, and retained unknown sections, or expose an explicitly metadata-only/incomplete result. Reject malformed/truncated sections and unsupported required compression rather than reporting a complete brush collection.

**Regression gate:** independent legacy raw/RLE brushes, modern sampled brushes, multiple descriptors, negative bounds, truncated sections, and unsupported variants.

### 38 — P2: CSH record boundaries are ignored

**Location:** `src/formats/csh.rs:87`, `:192`.

The shape size is assigned directly to `end_offset`, then passed to a parameter named `_size` that is never used. Path parsing uses an assumed count and unknown selectors consume only their selector bytes; subsequent record alignment is not enforced. Unsigned bounds subtraction can panic on reversed bounds. Only a Pascal-string unit test exercises this module.

**Suggested change:** verify the complete CSH layout against independently authored multi-shape files, constrain each shape to its declared payload, reuse the existing path-record parser where the formats match, and validate coordinates. Treat the assumed path-count layout as unverified until those fixtures pass.

**Regression gate:** multiple shapes, open/closed paths, unknown selectors, trailing fields, truncation, and reversed bounds. This finding is a code-backed boundary defect; full CSH interoperability was not executed.

### 39 — P2: ASE parsing ignores block lengths and writing truncates name lengths

**Location:** `src/formats/ase.rs:139`, `:355`, `:438`.

The parser stores block length as `_length`, so handlers cannot reliably skip legal extensions or detect crossing into the next block. Writer UTF-16 name lengths cast to u16 without a bounds check. ASE and CSH string readers remove interior NULs as well as terminators.

**Suggested change:** read each ASE block through a bounded slice, enforce valid group nesting, validate UTF-16 counts before emitting a header, and specify a narrow unknown-block policy.

**Regression gate:** blocks with extra/truncated data, nested/unbalanced groups, astral names and embedded NULs, and names exceeding 65,535 UTF-16 units.

### 40 — P2: PSD layer names are unnecessarily limited by Pascal fallback

**Location:** `src/io/writer.rs:160`, `:741`.

Every layer name must pass `write_pascal_string`, which errors when UTF-8 byte length exceeds 255 even though a Unicode `luni` name is also available. Non-ASCII UTF-8 bytes are replaced individually, creating multiple question marks per character in legacy strings.

**Suggested change:** generate a safely truncated legacy fallback using a defined encoding while retaining the full Unicode layer name in `luni`. Preserve original legacy bytes for resources where no Unicode companion exists.

**Regression gate:** long Unicode layer names, multibyte characters near the legacy boundary, and legacy non-ASCII names without a `luni` block.

### 41 — P3: write-side copying amplifies memory use

**Location:** `src/io/writer.rs:368`, `flatten_layers`, `:833`, `:969`; tagged-block assembly.

Writing clones the whole document, recursively clones layer trees during flattening, prepares all channel payloads at once, and copies modeled/raw tagged-block buffers into queues. `fallback_rgba` is allocated even when a composite is supplied. `build_layer_hierarchy` also repeatedly inserts at index zero, making wide sibling lists quadratic.

**Suggested change:** first allocate fallback pixels lazily and build/reverse sibling vectors instead of inserting at the front. Then use borrowed flattened records and avoid cloning immutable payloads during prewrite. Consider streaming output only after measuring the remaining peak-memory requirement; a new abstraction hierarchy is not needed for these fixes.

**Regression gate:** peak RSS and elapsed-time measurements on a large existing PSD and a synthetic wide/deep layer tree, with identical output semantics.

### 42 — P2: the test strategy can certify matching bugs

**Location:** `tests/ts_parity_test.rs`, `tests/integration_test.rs`, compression/descriptor unit tests, sample path helpers.

Many tests generate bytes with this crate and reread them with the same crate. Some sample parity assertions check only dimensions/depth/top-level count, and some skip composite data. A first parse that loses folder properties remains stable afterward. The ignored byte-exact test is not itself a bug—semantic output can legitimately differ—but its replacement must compare preserved semantics against an independent source. Some corpus helpers require an external sibling checkout even though fixtures exist in `tests/fixtures/samples`.

**Suggested change:** make the in-repo fixtures the default corpus; add independent golden wire bytes and an external parser gate. Keep self-roundtrips as one layer, not the oracle. Expand fixture coverage across depth, mode, compression, PSB, groups, masks, references, and edits. Add bounded fuzz targets after fixing the shared parser boundaries.

**Regression gate:** a clean checkout runs tests without unrelated repositories; the confirmed failures in this report become red tests before fixes. Separate parse acceptance, semantic preservation, actual edits, and rendered interoperability into distinct assertions.

### 43 — P2: documentation and old plans assert stronger support than current code provides

**Location:** `README.md`; prior compliance/correctness plans; public API docs.

The README claims Bitmap composite decoding and raw preservation of non-RGB layers, both contradicted by current paths. Its quick-start sets opacity to 128.0 although the writer clamps opacity to 0..1, producing fully opaque output. The July plan's unchecked tasks include fixes already present; it also elevates a local TypeScript implementation above independent format evidence. Copied tests/comments have reinforced incorrect compression and descriptor conventions.

**Suggested change:** publish a verified read/write/edit/preserve matrix for each mode/depth/compression combination. Fix opacity examples to `128.0 / 255.0`. Record which previous fixes actually landed, and tie format decisions to versioned fixtures and primary evidence. Explicitly separate parser/writer support from rendering and Photoshop edit compatibility.

**Regression gate:** compile runnable examples and assert their documented behavior; support claims link to relevant interoperability tests.

## Suggested implementation order

1. **Lock in failures before changing behavior:** commit small independent fixtures/probes for findings 01–05, 11–22, and 25–29. Correct tests that enshrine incompatible bytes.
2. **Make parsing bounded:** shared section constraints, allocation/recursion budgets, exact decompression, and checked dimensions/counts. These fixes protect all feature handlers.
3. **Correct core wire encoding:** ZIP/prediction, PSB context, descriptor enums/references, masks, clipping, and blending ranges.
4. **Make unchanged saves preserve data:** canonical folder records; native layer/composite channels; extra channels; record envelopes and unknown variants. Unsupported decoding must still permit safe opaque preservation where possible.
5. **Make edits reliable:** establish raw/typed precedence and dirty tracking, reconcile text/resources, and reject incompatible pixel geometry or unloaded content.
6. **Finish or narrow adjacent-format/options support:** ABR, CSH, ASE, unused public options, and the documented support matrix.
7. **Measure and simplify:** reduce redundant copies, share existing codecs, and add performance limits based on real corpus measurements.

Avoid a wholesale rewrite. The existing typed models and wire helpers are useful; repairing the shared boundaries and establishing an honest preservation contract will resolve more defects than adding another feature-specific layer.

## Acceptance criteria for a trustworthy release

- Independently authored PSD/PSB fixtures decode correctly for every advertised combination; independently decoded output agrees with original native samples and metadata.
- Unchanged saves preserve native sample precision, channels, groups, masks, clipping, supported semantics, and unsupported payloads.
- A documented edit to pixels/text/resources is observable after reopening, without manually discovering which hidden raw cache to clear.
- Unsupported operations return explicit errors; successful writing never knowingly emits a mismatched header/payload or silently replaces skipped content.
- Malformed files return bounded, contextual errors without panic, uncontrolled allocation, section overread, or silent truncation in strict mode.
- Group/mask/text/effect fixtures receive a real Photoshop open/edit/save check before claiming Photoshop interoperability. This audit did not perform that application-level validation.
- Fresh-checkout CI uses repository-owned fixtures; fuzzing and independent golden tests complement symmetric roundtrips.

## Executed probe observations

These are observed outputs, not proposed fixes:

```text
GROUP hidden=Some(false) opacity=Some(1.0) blend=Some(Normal)
  input: hidden=true, opacity=0.25, Multiply
EMPTY_GROUP None
  input: children=Some([])
CLIPPING None
  input: clipping=Some(1)
GRAY Ok([40, 40, 40, 255])
  input RGBA: [40,80,120,128]
CMYK Err(Io(UnexpectedEof))
RLE_SHORT Ok(()) [7, 0]
  input compressed row: [0,7], expected two decoded bytes
ZIP [99, 100, 98, 6, 0]
  input: [1,2,3]
ZIP_LENGTH Ok([1, 2, 3])
  requested output size: 1
PREDICTION_PANIC true
  compressed empty stream, width=2, height=1, depth=8
SECTION_OVERREAD Ok(1800)
  one-byte section, handler read_u16 consumes [7,8]
SECTION_PAST_EOF Ok(())
  declared 100-byte payload absent; handler reads nothing
MISMATCH_WRITE_OK true; read=Err(Io(UnexpectedEof))
  document width=2; composite width=1
DISABLED_MASK [40, 80, 120, 64]
  original alpha=128; disabled mask=64
LAYER16 [40,80,120,255,40,80,120,255]; raw=None
  second original pixel=[200,210,220,255]; use_raw_data=true
REFERENCE Ok(List([Class { name: "x", class_id: "Lyr " }]))
  input: Reference Offset with offset=42
PATH Ok(FilePath { sig: "txtu", path: U+F600 })
  input path: U+1F600
UNITS Ok(UnitDouble { units: "Pixels", value: 2.0 })
  input units: #XYZ
ENUM_BYTES contains modeEnmr followed by class structure
  input: ordinary Enum(Blnd, Nrml)
```

The one-pixel high-depth probe initially appeared correct because duplicated sample bytes happen to hide the bug. A second probe with two different pixels exposed it. This is why each regression fixture should contain values that distinguish competing implementations, not just uniform pixels or writer-generated defaults.
