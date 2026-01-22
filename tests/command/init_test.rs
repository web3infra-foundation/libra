//! Tests init command creating repository layout, configs, and database tables.

// use std::fs::File;
//
use std::fs;

use libra::internal::model::config;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::*;

pub fn verify_init(base_dir: &Path) {
    // List of subdirectories to verify
    let dirs = ["objects/pack", "objects/info", "info"];

    // Loop through the directories and verify they exist
    for dir in dirs {
        let dir_path = base_dir.join(dir);
        assert!(dir_path.exists(), "Directory {dir} does not exist");
    }

    // Additional file verification
    let files = ["description", "libra.db", "info/exclude"];

    for file in files {
        let file_path = base_dir.join(file);
        assert!(file_path.exists(), "File {file} does not exist");
    }
}
#[tokio::test]
#[serial]
/// Test the init function with no parameters
async fn test_init() {
    let target_dir = tempdir().unwrap().keep();
    // let _guard = ChangeDirGuard::new(target_dir.clone());

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
        ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();

    // Verify that the `.libra` directory exists
    let libra_dir = target_dir.join(".libra");
    assert!(libra_dir.exists(), ".libra directory does not exist");

    // Verify the contents of the other directory
    verify_init(libra_dir.as_path());
}

#[tokio::test]
#[serial]
/// Test the init function with a template directory
async fn test_init_template() {
    use std::fs;

    use tempfile::tempdir;

    // Create a temporary target directory for the new repo
    let target_dir = tempdir().unwrap().keep();

    // Create a temporary template directory
    let template_dir = tempdir().unwrap();

    // Set up template structure similar to Git template
    fs::create_dir_all(template_dir.path().join("objects/pack")).unwrap();
    fs::create_dir_all(template_dir.path().join("objects/info")).unwrap();
    fs::create_dir_all(template_dir.path().join("info")).unwrap();

    // Add description file in the template
    fs::write(
        template_dir.path().join("description"),
        "Template repository",
    )
    .unwrap();

    // Add info/exclude file in the template
    fs::write(template_dir.path().join("info/exclude"), "").unwrap();

    // Prepare init arguments with template path
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: Some(template_dir.path().to_str().unwrap().to_string()),
        shared: None,
        object_format: None,
        ref_format: None,
    };

    // Run the init function
    init(args).await.unwrap();

    // Verify that the `.libra` directory exists
    let libra_dir = target_dir.join(".libra");
    assert!(libra_dir.exists(), ".libra directory does not exist");

    // Verify the repository initialization structure
    verify_init(libra_dir.as_path());

    // --- Additional checks for template contents ---

    // Verify that description file is copied from template
    let description_path = libra_dir.join("description");
    assert!(
        description_path.exists(),
        "Template description file not copied"
    );

    // Verify that info/exclude file is copied from template
    let exclude_path = libra_dir.join("info/exclude");
    assert!(
        exclude_path.exists(),
        "Template info/exclude file not copied"
    );

    // Verify that objects subdirectories are copied from template
    assert!(
        libra_dir.join("objects/pack").exists(),
        "Template objects/pack directory not copied"
    );
    assert!(
        libra_dir.join("objects/info").exists(),
        "Template objects/info directory not copied"
    );
}

#[tokio::test]
#[serial]
/// Test the init function with an invalid template path
async fn test_init_with_invalid_template_path() {
    use tempfile::tempdir;

    // Create a temporary target directory for the new repo
    let target_dir = tempdir().unwrap().keep();

    // Provide a non-existent template path
    let invalid_template_path = "/path/to/nonexistent/template";

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: Some(invalid_template_path.to_string()),
        shared: None,
        object_format: None,ref_format: None,
    };

    // Run the init function and expect it to return an error
    let result = init(args).await;

    // Verify that the function returns an error due to invalid template path
    assert!(
        result.is_err(),
        "Init should fail when template path does not exist"
    );

    // Optionally, verify the error kind/message if your init function provides it
    if let Err(err) = result {
        // Uncomment and adjust depending on your error type
        // assert_eq!(err.kind(), Some(ExpectedErrorKind::NotFound));
        println!("Received expected error: {:?}", err);
    }
}

#[tokio::test]
#[serial]
/// Test the init function with the --bare flag
async fn test_init_bare() {
    let target_dir = tempdir().unwrap().keep();
    // let _guard = ChangeDirGuard::new(target_dir.clone());

    // Run the init function with --bare flag
    let args = InitArgs {
        bare: true,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();

    // Verify the contents of the other directory
    verify_init(target_dir.as_path());
}

#[tokio::test]
#[serial]
/// Test the init function with the --bare flag and an existing repository
async fn test_init_bare_with_existing_repo() {
    let target_dir = tempdir().unwrap().keep();

    // Initialize a bare repository
    let init_args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    init(init_args).await.unwrap(); // Execute init for bare repository

    // Simulate trying to reinitialize the bare repo
    let result = async {
        let args = InitArgs {
            bare: true,
            initial_branch: None,
            repo_directory: target_dir.to_str().unwrap().to_string(),
            quiet: false,
            template: None,
            shared: None,
            object_format: None,ref_format: None,
        };
        init(args).await
    };

    // Check for the error
    let err = result.await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists); // Check error type
    assert!(err.to_string().contains("Initialization failed")); // Check error message contains "Already initialized"
}

#[tokio::test]
#[serial]
/// Test the init function with an initial branch name
async fn test_init_with_initial_branch() {
    // Set up the test environment without a Libra repository
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let args = InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();

    // Verify the contents of the other directory
    verify_init(temp_path.path().join(".libra").as_path());

    // Verify the HEAD reference
    match Head::current().await {
        Head::Branch(current_branch) => {
            assert_eq!(current_branch, "main");
        }
        _ => panic!("should be branch"),
    };
}

#[tokio::test]
#[serial]
/// Test the init function with an invalid branch name
async fn test_init_with_invalid_branch() {
    // Cover all invalid branch name cases
    test_invalid_branch_name("master ").await;
    test_invalid_branch_name("master\t").await;
    test_invalid_branch_name("master\\").await;
    test_invalid_branch_name("master:").await;
    test_invalid_branch_name("master\"").await;
    test_invalid_branch_name("master?").await;
    test_invalid_branch_name("master*").await;
    test_invalid_branch_name("master[").await;
    test_invalid_branch_name("/master").await;
    test_invalid_branch_name("master/").await;
    test_invalid_branch_name("master.").await;
    test_invalid_branch_name("mast//er").await;
    test_invalid_branch_name("mast..er").await;
    test_invalid_branch_name("HEAD").await;
    test_invalid_branch_name("mast@{er").await;
    test_invalid_branch_name("").await;
    test_invalid_branch_name(".").await;
}

async fn test_invalid_branch_name(branch_name: &str) {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: Some(branch_name.to_string()),
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    let result = init(args).await;
    // Check for the error
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput); // Check error type
    assert!(err.to_string().contains("invalid branch name")); // Check error message contains "invalid branch name"
}

#[tokio::test]
#[serial]
/// Test the init function with [directory] parameter
async fn test_init_with_directory() {
    let target_dir = tempdir().unwrap().keep();

    // Create a test directory
    let test_dir = target_dir.join("test");

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: test_dir.to_str().unwrap().to_owned(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();

    // Verify that the `.libra` directory exists
    let libra_dir = test_dir.join(".libra");
    assert!(libra_dir.exists(), ".libra directory does not exist");

    // Verify the contents of the other directory
    verify_init(&libra_dir);
}

#[tokio::test]
#[serial]
/// Test the init function with invalid [directory] parameter
async fn test_init_with_invalid_directory() {
    let target_dir = tempdir().unwrap().keep();

    // Create a test file instead of a directory
    let test_dir = target_dir.join("test.txt");

    // Create a file with the same name as the test directory
    fs::File::create(&test_dir).unwrap();

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: test_dir.to_str().unwrap().to_owned(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    let result = init(args).await;

    // Check for the error
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput); // Check error type
    assert!(
        err.to_string()
            .contains("The target directory is not a directory")
    ); // Check error message
}

#[tokio::test]
#[serial]
/// Tests that repository initialization fails when lacking write permissions in the target directory
async fn test_init_with_unauthorized_directory() {
    let target_dir = tempdir().unwrap().keep();

    // Create a test directory
    let test_dir = target_dir.join("test");

    // Create a directory with restricted permissions
    fs::create_dir(&test_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&test_dir, fs::Permissions::from_mode(0o444)).unwrap();
    }
    #[cfg(windows)]
    {
        let mut perms = fs::metadata(&test_dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&test_dir, perms).unwrap();
    }

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: test_dir.to_str().unwrap().to_owned(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    let result = init(args).await;

    // Check for the error
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied); // Check error type
    assert!(
        err.to_string()
            .contains("The target directory is read-only")
    ); // Check error message
}

#[tokio::test]
#[serial]
/// Test the init function with the --quiet flag by using --show-output
async fn test_init_quiet() {
    let target_dir = tempdir().unwrap().keep();

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: true,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();

    // Verify that the `.libra` directory exists
    let libra_dir = target_dir.join(".libra");
    assert!(libra_dir.exists(), ".libra directory does not exist");

    // Verify the contents of the other directory
    verify_init(libra_dir.as_path());
}

/// Test the init function with the --shared flag
async fn test_valid_shared_mode(shared_mode: &str) {
    let target_dir = tempdir().unwrap().keep();

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: Some(shared_mode.to_string()),
        object_format: None,ref_format: None,
    };
    // Run the init function
    init(args).await.unwrap();
    // Verify that the '.libra' directory exists
    let libra_dir = target_dir.join(".libra");
    assert!(libra_dir.exists(), ".libra directory does not exist");
    // Check shared mode of '.libra' directory (Only Unix like os)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Verify the mode of pre-commit.sh
        let perms = std::fs::metadata(libra_dir.join("hooks/pre-commit.sh"))
            .unwrap()
            .permissions()
            .mode();
        match shared_mode {
            "true" | "group" => assert_eq!(perms & 0o777, 0o775),
            "all" | "world" | "everybody" => assert_eq!(perms & 0o777, 0o777),
            "false" | "umask" => (),
            mode if mode.starts_with('0') => {
                let expected = u32::from_str_radix(&mode[1..], 8).unwrap();
                assert_eq!(perms & 0o777, expected);
            }
            _ => panic!("Unsupported shared mode"),
        }
    }
}

async fn test_invalid_share_mode(shared_mode: &str) {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: Some(shared_mode.to_string()),
        object_format: None,ref_format: None,
    };

    let result = init(args).await;
    let err = result.unwrap_err();

    // Verify the type of error
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[tokio::test]
#[serial]
/// Test the init function with valid shared mode
async fn test_init_with_valid_shared_mode() {
    // Test all types of valid shared modes
    test_valid_shared_mode("true").await;
    test_valid_shared_mode("false").await;
    test_valid_shared_mode("umask").await;
    test_valid_shared_mode("group").await;
    test_valid_shared_mode("all").await;
    test_valid_shared_mode("world").await;
    test_valid_shared_mode("everybody").await;
    test_valid_shared_mode("0777").await;
}

#[tokio::test]
#[serial]
/// Test the init function with invalid shared mode
async fn test_init_with_invalid_shared_mode() {
    test_invalid_share_mode("invalid").await;
    test_invalid_share_mode("mygroup").await;
    test_invalid_share_mode("1234").await;
    test_invalid_share_mode("0888").await;
    test_invalid_share_mode("12345").await;
}

#[tokio::test]
#[serial]
/// Test init with a valid object format ('sha1' and 'sha256' are supported)
async fn test_init_with_valid_object_format_sha1() {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: Some("sha1".to_string()),
    };
    // This should succeed
    let result = init(args).await;
    assert!(
        result.is_ok(),
        "init with --object-format sha1 should succeed"
    );

    // Verify that the config file contains the correct object format
    let db_path = target_dir.join(".libra/libra.db");
    // Connect to the existing database instead of creating a new one
    let conn = sea_orm::Database::connect(format!("sqlite://{}", db_path.to_str().unwrap()))
        .await
        .unwrap();
    let config_entry = config::Entity::find()
        .filter(config::Column::Configuration.eq("core"))
        .filter(config::Column::Key.eq("objectformat"))
        .one(&conn)
        .await
        .unwrap();
    assert_eq!(config_entry.unwrap().value, "sha1");
}

#[tokio::test]
#[serial]
/// Test init with a valid object format ('sha256') and verify it's saved to config.
async fn test_init_with_valid_object_format_sha256() {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: true, // Use quiet to reduce test output noise
        template: None,
        shared: None,
        object_format: Some("sha256".to_string()),
    };
    // This should succeed
    let result = init(args).await;
    assert!(
        result.is_ok(),
        "init with --object-format sha256 should succeed"
    );

    // Verify that the config file contains the correct object format
    let db_path = target_dir.join(".libra/libra.db");
    let conn = sea_orm::Database::connect(format!("sqlite://{}", db_path.to_str().unwrap()))
        .await
        .unwrap();
    let config_entry = config::Entity::find()
        .filter(config::Column::Configuration.eq("core"))
        .filter(config::Column::Key.eq("objectformat"))
        .one(&conn)
        .await
        .unwrap();
    assert_eq!(config_entry.unwrap().value, "sha256");
}

#[tokio::test]
#[serial]
/// Test init with an invalid object format (e.g., 'md5')
async fn test_init_with_invalid_object_format() {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: Some("md5".to_string()),
        ref_format: None,
    };
    // This should fail with a generic invalid format error
    let result = init(args).await;
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("unsupported object format"));
}

#[tokio::test]
#[serial]
/// Test init with a custom ref format and verify it's saved to config.
async fn test_init_with_ref_format() {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: Some("dev".to_string()),
#[cfg(unix)]
/// Init should fail when the parent directory is not writable (permission denied)
async fn test_init_fails_when_parent_not_writable() {
    use std::os::unix::fs::PermissionsExt;
    // Skip test if running as root - root can bypass permission restrictions
    use std::process::Command;
    let output = Command::new("id")
        .arg("-u")
        .output()
        .expect("failed to execute id -u");
    let uid = String::from_utf8(output.stdout).expect("failed to parse uid");
    if uid.trim() == "0" {
        return;
    }

    let dir = tempdir().unwrap();
    let path = dir.path();

    let original_perms = std::fs::metadata(path).unwrap().permissions();

    let mut perms = original_perms.clone();
    perms.set_mode(0o555);
    std::fs::set_permissions(path, perms).unwrap();

    // Attempt to initialize a repo inside a non-writable parent directory
    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: path.join("repo").to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,
    };

    // Expect init to fail with PermissionDenied
    let err = init(args).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    // Restore original permissions so tempdir can clean up properly
    std::fs::set_permissions(path, original_perms).unwrap();
}

#[tokio::test]
#[serial]
/// Init should create a hooks/pre-commit.sh file that is present and non-empty
async fn test_init_creates_hooks_and_precommit() {
    let target_dir = tempfile::tempdir().unwrap().keep();

    let args = InitArgs {
        bare: false,
        initial_branch: None,
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
        ref_format: Some(RefFormat::Strict),
    };

    // Run the init function with strict ref format
    let result = init(args).await;
    assert!(result.is_ok(), "init with --ref-format=strict should succeed");

    // Verify that the config contains the initrefformat entry
    let db_path = target_dir.join(".libra/libra.db");
    let conn = sea_orm::Database::connect(format!("sqlite://{}", db_path.to_str().unwrap()))
        .await
        .unwrap();
    let config_entry = config::Entity::find()
        .filter(config::Column::Configuration.eq("core"))
        .filter(config::Column::Key.eq("initrefformat"))
        .one(&conn)
        .await
        .unwrap();
    assert_eq!(config_entry.unwrap().value, "strict");
}

#[tokio::test]
#[serial]
/// Test init rejects invalid branch names according to ref format validation
async fn test_init_with_invalid_ref_format() {
    let target_dir = tempdir().unwrap().keep();
    let args = InitArgs {
        bare: false,
        initial_branch: Some("invalid branch".to_string()), // contains space
        repo_directory: target_dir.to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,ref_format: None,
        ref_format: Some(RefFormat::Strict),
    };

    // This should fail due to invalid branch name
    let result = init(args).await;
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("branch name contains invalid characters"));
}
