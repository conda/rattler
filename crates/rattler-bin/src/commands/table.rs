//! Minimal text table shared by the commands that print aligned columns.

/// A single table cell: the text as it should be displayed and the plain text
/// used to compute the column width. The two are kept apart because ANSI
/// escape codes in styled text occupy no terminal columns but would otherwise
/// be counted.
pub struct Cell {
    styled: String,
    plain: String,
}

impl Cell {
    /// A cell without any styling.
    pub fn plain(text: impl Into<String>) -> Self {
        let plain = text.into();
        Self {
            styled: plain.clone(),
            plain,
        }
    }

    /// A cell displayed as `styled` whose width is that of `plain`.
    pub fn styled(styled: impl ToString, plain: impl Into<String>) -> Self {
        Self {
            styled: styled.to_string(),
            plain: plain.into(),
        }
    }

    /// The number of terminal columns the cell occupies.
    fn width(&self) -> usize {
        self.plain.chars().count()
    }
}

/// A table with left-aligned columns, two spaces of inter-column padding and
/// no trailing whitespace. Every column is as wide as its longest cell.
#[derive(Default)]
pub struct Table {
    rows: Vec<Vec<Cell>>,
    widths: Vec<usize>,
    indent: usize,
}

impl Table {
    /// Creates an empty table without a header row.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a table whose first row is `header`, displayed in bold.
    pub fn with_header(header: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut table = Self::new();
        table.add_row(header.into_iter().map(|field| {
            let plain = field.into();
            let styled = console::style(&plain).bold().to_string();
            Cell::styled(styled, plain)
        }));
        table
    }

    /// Indents every line of the table by `indent` spaces.
    pub fn with_indent(mut self, indent: usize) -> Self {
        self.indent = indent;
        self
    }

    /// Appends a row. Rows may have differing numbers of cells.
    pub fn add_row(&mut self, row: impl IntoIterator<Item = Cell>) {
        let row: Vec<Cell> = row.into_iter().collect();
        if self.widths.len() < row.len() {
            self.widths.resize(row.len(), 0);
        }
        for (width, cell) in self.widths.iter_mut().zip(&row) {
            *width = (*width).max(cell.width());
        }
        self.rows.push(row);
    }

    /// Renders the table, one line per row, each ending in a newline.
    fn render(&self) -> String {
        let mut out = String::new();
        for row in &self.rows {
            let mut line = " ".repeat(self.indent);
            for (i, cell) in row.iter().enumerate() {
                line.push_str(&cell.styled);
                // Don't pad the last cell, that would only add trailing
                // whitespace.
                if i + 1 < row.len() {
                    let padding = self.widths[i].saturating_sub(cell.width());
                    line.push_str(&" ".repeat(padding + 2));
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// Prints the table to stdout.
    pub fn print(&self) {
        print!("{}", self.render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columns_align_to_longest_cell() {
        let mut table = Table::new();
        table.add_row([Cell::plain("a"), Cell::plain("bb")]);
        table.add_row([Cell::plain("cccc"), Cell::plain("d")]);
        assert_eq!(table.render(), "a     bb\ncccc  d\n");
    }

    #[test]
    fn test_widths_count_characters_not_bytes() {
        let mut table = Table::new();
        table.add_row([Cell::plain("héllo"), Cell::plain("x")]);
        table.add_row([Cell::plain("hello"), Cell::plain("y")]);
        // Both names are five characters wide, so both rows pad identically.
        assert_eq!(table.render(), "héllo  x\nhello  y\n");
    }

    #[test]
    fn test_styled_cells_pad_by_plain_width() {
        let mut table = Table::new();
        table.add_row([
            Cell::styled("\u{1b}[32mok\u{1b}[0m", "ok"),
            Cell::plain("1"),
        ]);
        table.add_row([Cell::plain("nope"), Cell::plain("2")]);
        assert_eq!(table.render(), "\u{1b}[32mok\u{1b}[0m    1\nnope  2\n");
    }

    #[test]
    fn test_no_trailing_whitespace() {
        let mut table = Table::new();
        table.add_row([Cell::plain("long-name"), Cell::plain("z")]);
        table.add_row([Cell::plain("x"), Cell::plain("")]);
        for line in table.render().lines() {
            assert_eq!(line, line.trim_end());
        }
    }

    #[test]
    fn test_indent() {
        let mut table = Table::new().with_indent(2);
        table.add_row([Cell::plain("a"), Cell::plain("b")]);
        assert_eq!(table.render(), "  a  b\n");
    }
}
