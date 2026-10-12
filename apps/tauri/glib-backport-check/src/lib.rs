//! Optimized regression qualification for the GTK3-compatible GLib backport.
#[cfg(test)]
mod tests {
    use glib::variant::ToVariant;

    #[test]
    fn variant_str_iterator_output_pointer_is_valid_under_optimization() {
        let values = ["zero", "one", "two", "three", "four", "five"];
        let variant = values.to_variant();
        assert_eq!(
            variant.array_iter_str().unwrap().collect::<Vec<_>>(),
            values
        );
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.next(), Some("zero"));
        assert_eq!(iter.next_back(), Some("five"));
        assert_eq!(iter.nth(1), Some("two"));
        assert_eq!(iter.nth_back(1), Some("three"));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        assert_eq!(variant.array_iter_str().unwrap().last(), Some("five"));
        assert_eq!(variant.array_iter_str().unwrap().nth(usize::MAX), None);
        assert_eq!(variant.array_iter_str().unwrap().nth_back(usize::MAX), None);
        let empty = Vec::<String>::new().to_variant();
        assert_eq!(empty.array_iter_str().unwrap().next(), None);
        assert_eq!(empty.array_iter_str().unwrap().next_back(), None);
        assert_eq!(empty.array_iter_str().unwrap().last(), None);
    }
}
