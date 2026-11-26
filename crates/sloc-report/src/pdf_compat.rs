//! Compatibility shim presenting the printpdf 0.7 high-level "layer" drawing API on
//! top of printpdf's op-based model (0.9 through 0.13).
//!
//! The pure-Rust PDF renderer in [`crate`] was written against printpdf 0.7's imperative
//! `PdfLayerReference` API (`use_text`, `set_fill_color`, `add_polygon`, `get_layer`, …).
//! printpdf 0.9 replaced that model with a declarative `Vec<Op>` per page, and 0.10 through
//! 0.13 kept that op surface source-compatible. This module re-implements exactly the subset
//! of the old surface the renderer uses, accumulating the equivalent `Op`s, so the ~50
//! rendering helpers compile and produce identical output without being rewritten one-by-one.
//!
//! Builtin (Helvetica) fonts need no explicit registration: printpdf's serializer discovers
//! them from the `SetFont` ops emitted here and adds the resource automatically.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use anyhow::{Context, Result};

// Value types whose API is unchanged from printpdf 0.7 through 0.11 — re-exported so the
// renderer can keep importing them via `crate::pdf_compat::{Color, Mm, Rgb, BuiltinFont}`.
pub use printpdf::{BuiltinFont, Color, Mm, Rgb};

// Types used only to build ops inside this module.
use printpdf::{
    LinePoint, Op, PaintMode, PdfFontHandle, PdfPage, PdfSaveOptions, PdfWarnMsg, Point, Polygon,
    PolygonRing, Pt, TextItem, WindingOrder,
};

// The renderer refers to these by their 0.7 names; alias them to the shim/std types.
pub type PdfPageIndex = usize;
pub type PdfLayerIndex = usize;
/// In 0.7 a font handle was an `IndirectFontRef`; here a builtin font is all we use, and
/// [`BuiltinFont`] is `Copy`, so the handle is simply the builtin font itself.
pub type IndirectFontRef = BuiltinFont;

/// One page's accumulated drawing operations plus its dimensions (mm).
struct PageBuf {
    w: f32,
    h: f32,
    ops: Rc<RefCell<Vec<Op>>>,
}

struct DocInner {
    title: String,
    pages: Vec<PageBuf>,
}

/// Stand-in for printpdf 0.7's `PdfDocumentReference`. Uses interior mutability so pages can
/// be appended through a shared `&self` reference, matching the old reference-counted handle.
#[derive(Clone)]
pub struct PdfDocumentReference {
    inner: Rc<RefCell<DocInner>>,
}

/// Stand-in for printpdf 0.7's `PdfDocument` factory.
pub struct PdfDocument;

impl PdfDocument {
    /// Mirror of `printpdf::PdfDocument::new(title, width, height, layer_name)` — creates the
    /// document with its first page and returns `(doc, page_index, layer_index)`.
    // Intentionally returns a tuple, not `Self`: this reproduces printpdf 0.7's factory
    // signature so the renderer's `let (doc, page, layer) = PdfDocument::new(..)` is unchanged.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        title: impl Into<String>,
        width: Mm,
        height: Mm,
        _initial_layer_name: &str,
    ) -> (PdfDocumentReference, PdfPageIndex, PdfLayerIndex) {
        let first = PageBuf {
            w: width.0,
            h: height.0,
            ops: Rc::new(RefCell::new(Vec::new())),
        };
        let inner = DocInner {
            title: title.into(),
            pages: vec![first],
        };
        let doc = PdfDocumentReference {
            inner: Rc::new(RefCell::new(inner)),
        };
        (doc, 0, 0)
    }
}

impl PdfDocumentReference {
    /// Builtin fonts require no registration in printpdf's op model — the handle is the font itself.
    /// Kept fallible to match the 0.7 signature the renderer calls (`.ok()?` / `.map_err(..)?`).
    // Signature intentionally mirrors printpdf 0.7's fallible `&self` method so the renderer's
    // call sites keep compiling unchanged; the implementation is trivially infallible.
    #[allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        clippy::missing_const_for_fn
    )]
    pub fn add_builtin_font(&self, font: BuiltinFont) -> Result<IndirectFontRef> {
        Ok(font)
    }

    /// Append a page and return its `(page_index, layer_index)`.
    pub fn add_page(&self, width: Mm, height: Mm, _name: &str) -> (PdfPageIndex, PdfLayerIndex) {
        let mut inner = self.inner.borrow_mut();
        inner.pages.push(PageBuf {
            w: width.0,
            h: height.0,
            ops: Rc::new(RefCell::new(Vec::new())),
        });
        (inner.pages.len() - 1, 0)
    }

    /// Obtain a handle to an existing page so its layer (op buffer) can be drawn onto.
    pub fn get_page(&self, page: PdfPageIndex) -> PdfPageHandle {
        let ops = self.inner.borrow().pages[page].ops.clone();
        PdfPageHandle { ops }
    }

    /// Serialize every accumulated page to a real printpdf document and write the bytes.
    pub fn save<W: Write>(&self, writer: &mut W) -> Result<()> {
        let inner = self.inner.borrow();
        let mut doc = printpdf::PdfDocument::new(&inner.title);
        let pages: Vec<PdfPage> = inner
            .pages
            .iter()
            .map(|p| PdfPage::new(Mm(p.w), Mm(p.h), p.ops.borrow().clone()))
            .collect();
        doc.with_pages(pages);
        let mut warnings: Vec<PdfWarnMsg> = Vec::new();
        let bytes = doc.save(&PdfSaveOptions::default(), &mut warnings);
        writer
            .write_all(&bytes)
            .context("failed to write PDF bytes")?;
        Ok(())
    }
}

/// Handle to a single page, from which the (single) layer is retrieved.
pub struct PdfPageHandle {
    ops: Rc<RefCell<Vec<Op>>>,
}

impl PdfPageHandle {
    pub fn get_layer(&self, _layer: PdfLayerIndex) -> PdfLayerReference {
        PdfLayerReference {
            ops: self.ops.clone(),
        }
    }
}

/// Stand-in for printpdf 0.7's `PdfLayerReference` — a cheap clonable handle to one page's
/// op buffer. Drawing methods push the equivalent printpdf `Op`s in call order, so the
/// resulting content stream matches the old imperative draw sequence.
#[derive(Clone)]
pub struct PdfLayerReference {
    ops: Rc<RefCell<Vec<Op>>>,
}

impl PdfLayerReference {
    /// Set the current fill colour (persists in the graphics state for following draws/text).
    pub fn set_fill_color(&self, col: Color) {
        self.ops.borrow_mut().push(Op::SetFillColor { col });
    }

    /// Draw one run of text at an absolute `(x, y)` (bottom-left origin, mm) in the given
    /// builtin font and point size, using the current fill colour. Equivalent to 0.7's
    /// `PdfLayerReference::use_text`.
    pub fn use_text(
        &self,
        text: impl Into<String>,
        font_size: f32,
        x: Mm,
        y: Mm,
        font: IndirectFontRef,
    ) {
        let mut ops = self.ops.borrow_mut();
        ops.push(Op::StartTextSection);
        ops.push(Op::SetTextCursor {
            pos: Point {
                x: x.into_pt(),
                y: y.into_pt(),
            },
        });
        ops.push(Op::SetFont {
            font: PdfFontHandle::Builtin(font),
            size: Pt(font_size),
        });
        ops.push(Op::ShowText {
            items: vec![TextItem::Text(text.into())],
        });
        ops.push(Op::EndTextSection);
    }

    /// Fill an axis-aligned rectangle with `color`. Replaces the 0.7 `set_fill_color` +
    /// `add_polygon(Polygon { rings: vec![vec![(Point, bool)]], .. })` idiom.
    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        self.set_fill_color(Color::Rgb(color));
        let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
        let points = corners
            .iter()
            .map(|&(px, py)| LinePoint {
                p: Point::new(Mm(px), Mm(py)),
                bezier: false,
            })
            .collect();
        self.ops.borrow_mut().push(Op::DrawPolygon {
            polygon: Polygon {
                rings: vec![PolygonRing { points }],
                mode: PaintMode::Fill,
                winding_order: WindingOrder::NonZero,
            },
        });
    }
}
