#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use crate::directory::Ignores;
    
    #[test]
    fn test_ignores_new_creates_valid_glob_set() {
        let patterns = vec![
            "*.log".to_string(),
            "build".to_string(),
            "docs/**/*.md".to_string(),
        ];
        
        let ignores = Ignores::new(patterns.clone());
        
        // Verify patterns stored correctly
        assert_eq!(ignores.patterns, patterns);
        
        // Verify glob matching works
        assert!(ignores.is_ignored(&PathBuf::from("error.log")));
        assert!(ignores.is_ignored(&PathBuf::from("build")));
        assert!(ignores.is_ignored(&PathBuf::from("docs/api/readme.md")));
        
        // Verify non-matches don't match
        assert!(!ignores.is_ignored(&PathBuf::from("src/main.rs")));
        assert!(!ignores.is_ignored(&PathBuf::from("docs/readme.txt")));
    }
    
    #[test]
    fn test_default_ignores() {
        let ignores = Ignores::default();
        
        // Verify common defaults are included
        assert!(ignores.is_ignored(&PathBuf::from(".git")));
        assert!(ignores.is_ignored(&PathBuf::from(".rev")));
        assert!(ignores.is_ignored(&PathBuf::from("target")));
        
        // Verify common binary/object files are included
        assert!(ignores.is_ignored(&PathBuf::from("file.class")));
        assert!(ignores.is_ignored(&PathBuf::from("obj/file.o")));
        assert!(ignores.is_ignored(&PathBuf::from("lib/file.so")));
    }
    
    #[test]
    fn test_ignores_handles_invalid_patterns() {
        // Create ignores with a mix of valid and invalid patterns
        // This test primarily ensures we don't panic on invalid patterns
        let patterns = vec![
            "*.log".to_string(),
            "[".to_string(),  // Invalid - unclosed character class
            "build".to_string(),
        ];
        
        // Should not panic
        let ignores = Ignores::new(patterns);
        
        // Valid patterns should still work
        assert!(ignores.is_ignored(&PathBuf::from("error.log")));
        assert!(ignores.is_ignored(&PathBuf::from("build")));
    }
}