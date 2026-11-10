//! Splitting a list into pages for display.

/// How many pages `len` items fill at `per_page` a page. A partly filled
/// last page counts. Zero when `per_page` is zero.
pub fn page_count(len: usize, per_page: usize) -> usize {
    if per_page == 0 {
        return 0;
    }
    len.div_ceil(per_page)
}

/// The items on page `page` (counting from 1), `per_page` to a page. Empty
/// for page 0, for a page past the end, and when `per_page` is zero.
pub fn page<T: Clone>(items: &[T], page: usize, per_page: usize) -> Vec<T> {
    if page == 0 || per_page == 0 || page > page_count(items.len(), per_page) {
        return Vec::new();
    }
    let start = (page - 1) * per_page;
    let end = (start + per_page).min(items.len());
    items[start..end].to_vec()
}

/// "Page 2 of 5", for the footer.
pub fn footer(page: usize, len: usize, per_page: usize) -> String {
    format!("Page {page} of {}", page_count(len, per_page).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_show() {
        assert!(page::<u8>(&[], 1, 10).is_empty());
        assert_eq!(footer(1, 0, 10), "Page 1 of 1");
    }
}
