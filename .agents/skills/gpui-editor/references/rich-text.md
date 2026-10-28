# Rich text

**Contents:** [What exists vs not](#what-exists-and-what-doesnt) · [Document model](#the-document-model) · [Rendering](#rendering-styled-runs-per-line) · [Mixed sizes & baselines](#mixed-font-sizes--baselines) · [Editing semantics](#editing-semantics-of-styled-ranges) · [Non-text content](#non-text-inline-content) · [Serialization](#serialization) · [A staged path](#a-staged-path)

Honest framing first: **neither reference implementation is a rich text editor.** Zed and gpui-component are monospace plain/code editors. But nearly every mechanism rich text needs already exists in this skill — what's genuinely new is the *document model* and the editing semantics of styled ranges. GPUI itself is agnostic: text is shaped from `TextRun`s, which already carry font (family/weight/style), color, background, underline, strikethrough per byte range.

## What exists, and what doesn't

| Need | Status |
|---|---|
| Bold/italic/color/underline/strike spans | ✅ `TextRun` per span — highlighting.md §Styled runs |
| Mixed fonts per span | ✅ `TextRun.font` |
| Mixed font **sizes** in one line | ⚠️ manual — see §Mixed sizes |
| Wrapping with mixed-width text | ✅ `LineWrapper` measures real glyphs — multiline-and-wrap.md |
| Inline images/mentions/chips | ✅ inline-widget fragments — decoration.md §Inline widgets |
| Block structure (headings, lists, quotes, code blocks) | your document model + per-block styling |
| Style-carrying clipboard, serialization | you |
| The document model itself | you |

## The document model

Two workable shapes; pick by ambition:

**A. Text + span table (recommended start).** One text buffer (rope) + an out-of-band list of styled ranges:

```rust
struct RichDoc {
    text: Rope,                                  // paragraphs separated by \n
    spans: Vec<(AnchorRange, InlineStyle)>,      // bold 10..24, link 30..38 — may overlap
    blocks: Vec<(AnchorRange, BlockStyle)>,      // heading/list-item/quote/code per paragraph range
}
bitflags InlineStyle: bold, italic, underline, strike, code; plus link(url), color…
```

This is literally the decoration-collection mechanism (decoration.md) promoted to *be* the document: anchor-stored ranges that track edits, with stickiness rules per style (see §Editing semantics). The entire plain-text editing core — cursor, selection, IME, undo, search — works unchanged because the text is still just text.

**B. Tree of nodes** (paragraphs → runs; the DOM/ProseMirror shape). More faithful for deeply structured docs (nested lists, tables), but every piece of editor machinery in this skill (byte offsets, shape-per-line, search, IME ranges) now needs a mapping layer between tree positions and flat offsets. Only take this when structure demands it — you can convert A→B later behind the same editor surface.

## Rendering: styled runs per line

Per visible line: gather spans intersecting the line, split at boundaries, merge overlaps (bold∩italic → bold-italic — style merge is per *attribute*, unlike the first-wins rule for decorations), emit `TextRun`s, shape, paint. Identical pipeline to syntax highlighting (highlighting.md §Merging) with the span table as the style source; search-match and selection styling layer on top exactly as before. Block styles wrap the line: heading → bigger font size for that line's shaping + spacing above; list item → hanging indent (wrap with `boundary.next_indent`); quote → left border quad + inset; code block → background quad + monospace font runs.

## Mixed font sizes & baselines

The one primitive gap: `shape_line(text, font_size, runs, ..)` takes **a single font size per call** — runs vary font/weight/color but not size. Per-line uniform size (headings!) is therefore free; genuinely mixed sizes *within* one line require shaping each size-segment as its own `ShapedLine` and laying them out as fragments on a shared baseline:

- line height = max segment line height; align segments by baseline (ascent), not top;
- cursor/hit-testing sums across fragments (the inline-widget fragment walk in decoration.md already does this);
- wrap by measuring per-fragment (the `LineFragment` path in multiline-and-wrap.md).

This is real work but confined to the line-layout layer. Most rich-text features (docs, notes, comments) live happily with per-paragraph sizes + inline weight/style — defer intra-line sizes.

## Editing semantics of styled ranges

Where rich text is actually hard — decide these explicitly:

- **Typing at span edges:** typing at the end of a bold span — bold or not? Convention: *inherit the style to the left* (span grows at its end, not at its start) — note this is the **opposite** stickiness default from decorations (search hits shouldn't grow; formatting should).
- **The pending style:** toggling bold with a collapsed cursor sets a *pending style* applied to the next insertion (and shown on the toolbar), cleared by cursor movement.
- **Toggle over a selection:** if the whole selection is bold → unbold; else → bold (the "any/all" rule). Implement as span-table surgery: split/merge ranges; normalize (coalesce adjacent equal-style spans) after every edit.
- **Undo** must capture span-table changes alongside text changes — same transaction (undo.md); style toggles are undoable edits with zero text delta.
- **Paste:** plain text inherits the style at the insertion point; rich paste (your own format via clipboard metadata, or parsed HTML) splices both text and spans.
- **Newline in a heading** → the new paragraph usually reverts to body style (block styles don't inherit; inline styles do).

## Non-text inline content

Mentions (@user pills), inline images, emoji shortcodes: an inline-widget fragment (decoration.md) backed by a placeholder in the text (e.g. an object-replacement char `\u{FFFC}` covering 1+ bytes) so offsets, selection, backspace ("delete the pill atomically" — treat it as one grapheme-like unit in boundary functions), and serialization stay coherent.

## Serialization

Span-table docs serialize naturally to markdown (lossy but portable) or a JSON span format (lossless). Keep the on-disk format decoupled from the in-memory model; convert at load/save. If markdown *is* the source of truth, consider whether you want a rich editor at all vs a syntax-highlighted markdown editor with live preview — the latter is buildable entirely from highlighting.md + decoration.md and is far cheaper.

## A staged path

1. Plain multi-line editor (core-editing.md + multiline-and-wrap.md + undo.md).
2. Span table + rendering: bold/italic/color spans, toolbar toggles, pending style. Cheap — it's the decoration pipeline with edit rules.
3. Block styles: per-paragraph size/indent/background.
4. Inline widgets: links with hover, mentions, images.
5. Only if truly needed: intra-line font sizes (baseline layout) and/or a node-tree model.
