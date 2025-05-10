use std::path::PathBuf;

#[cfg(test)]
mod tests {
    use crate::show_general_help;
    use crate::get_command_help;
    
    #[test]
    fn test_general_help_returns_content() {
        let help = show_general_help();
        
        // Ensure the help content contains expected sections
        assert!(help.contains("revtool - A lightweight version control system"));
        assert!(help.contains("Starting a new project:"));
        assert!(help.contains("Making changes:"));
        assert!(help.contains("Working with branches:"));
        assert!(help.contains("Comparing branches:"));
    }
    
    #[test]
    fn test_command_help_returns_content_for_valid_commands() {
        // Test a few known commands
        let init_help = get_command_help("init");
        assert!(init_help.is_some());
        
        let status_help = get_command_help("status");
        assert!(status_help.is_some());
        
        let snap_help = get_command_help("snap");
        assert!(snap_help.is_some());
        
        // Ensure help content contains expected information
        let init_help_content = init_help.unwrap();
        assert!(init_help_content.contains("Initialize a new revision control repository"));
        
        let status_help_content = status_help.unwrap();
        assert!(status_help_content.contains("Show the current status of the working directory"));
        
        let snap_help_content = snap_help.unwrap();
        assert!(snap_help_content.contains("Create a new snapshot"));
    }
    
    #[test]
    fn test_command_help_returns_none_for_invalid_commands() {
        assert!(get_command_help("invalid_command").is_none());
        assert!(get_command_help("not_a_real_command").is_none());
    }
    
    #[test]
    fn test_command_help_handles_case_insensitivity() {
        // Command names should be case-insensitive
        assert!(get_command_help("INIT").is_some());
        assert!(get_command_help("Diff").is_some());
        assert!(get_command_help("sTaTuS").is_some());
    }
}