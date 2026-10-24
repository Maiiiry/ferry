pub fn answer() -> u32 {
    42
}

#[cfg(test)]
mod tests {
    #[test]
    fn answer_is_stable() {
        assert_eq!(super::answer(), 42);
    }
}
