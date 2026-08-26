//! A tiny indenting XML writer for the OOXML parts.
//!
//! Kept deliberately small: OOXML parts are regular, so a ~80-line builder
//! with escaping is easier to audit than a full XML library.

/// Escape text/attribute content per XML 1.0.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// A minimal XML writer producing one document (with prolog) in memory.
#[derive(Debug, Default)]
pub struct Xml {
    buf: String,
    indent: usize,
}

impl Xml {
    /// Create a writer that already carries the XML prolog.
    pub fn new() -> Self {
        Self {
            buf: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n"),
            indent: 0,
        }
    }

    /// Open an element with attributes (may be empty).
    pub fn start(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.push_open(name, attrs);
        self.indent += 1;
        self.buf.push('\n');
    }

    /// Close an element opened by [`Xml::start`].
    pub fn end(&mut self, name: &str) {
        self.indent -= 1;
        self.write_indent();
        self.buf.push_str("</");
        self.buf.push_str(name);
        self.buf.push_str(">\n");
    }

    /// A self-closing element.
    pub fn leaf(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        for (key, value) in attrs {
            self.buf.push(' ');
            self.buf.push_str(key);
            self.buf.push_str("=\"");
            self.buf.push_str(&escape(value));
            self.buf.push('"');
        }
        self.buf.push_str("/>\n");
    }

    /// Escaped text content (no line break or indentation around it).
    pub fn text(&mut self, text: &str) {
        self.buf.push_str(&escape(text));
    }

    /// `name` containing the escaped `text` as its only content.
    pub fn text_elem(&mut self, name: &str, text: &str) {
        self.start(name, &[]);
        self.text(text);
        self.end(name);
    }

    /// Emit the document and return the final string.
    pub fn into_string(self) -> String {
        self.buf
    }

    fn push_open(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.write_indent();
        self.buf.push('<');
        self.buf.push_str(name);
        for (key, value) in attrs {
            self.buf.push(' ');
            self.buf.push_str(key);
            self.buf.push_str("=\"");
            self.buf.push_str(&escape(value));
            self.buf.push('"');
        }
        self.buf.push('>');
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.buf.push_str("  ");
        }
    }
}
