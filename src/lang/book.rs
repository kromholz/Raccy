use std::collections::HashMap;
use std::sync::OnceLock;

pub type Book = HashMap<String, Vec<String>>;

pub const CS: &str = include_str!("../../voice/cs.toml");
pub const EN: &str = include_str!("../../voice/en.toml");

pub fn cs() -> &'static Book {
    static BOOK: OnceLock<Book> = OnceLock::new();
    BOOK.get_or_init(|| read(CS))
}

pub fn en() -> &'static Book {
    static BOOK: OnceLock<Book> = OnceLock::new();
    BOOK.get_or_init(|| read(EN))
}

// A file that does not read leaves the book empty, so every line shows as missing; the tests keep that from shipping.
fn read(source: &str) -> Book {
    let mut book = Book::new();
    if let Ok(table) = source.parse::<toml::Table>() {
        collect("", &toml::Value::Table(table), &mut book);
    }
    book
}

fn collect(path: &str, value: &toml::Value, book: &mut Book) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                collect(&path, value, book);
            }
        }
        toml::Value::String(line) => {
            book.insert(path.to_string(), vec![line.clone()]);
        }
        toml::Value::Array(lines) => {
            book.insert(path.to_string(), lines.iter().filter_map(|l| l.as_str().map(str::to_string)).collect());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_voice_files_read() {
        for (name, source) in [("cs", CS), ("en", EN)] {
            if let Err(e) = source.parse::<toml::Table>() {
                panic!("voice/{name}.toml: {e}");
            }
        }
        assert!(cs().len() > 200 && en().len() > 200);
        assert!(cs().values().all(|lines| !lines.is_empty()), "a key with no lines");
    }
}
