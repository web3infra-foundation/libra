//! Unit tests for mercury adapters

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;
    use sea_orm::ActiveValue;
    
    #[test]
    fn test_convert_git_commit_model() {
        // Create a mercury GitCommitModel
        let mercury_model = mercury::internal::model::sea_models::git_commit::Model {
            id: 1,
            repo_id: 123,
            commit_id: "commit123".to_string(),
            tree: "tree123".to_string(),
            parents_id: r#"["parent1", "parent2"]"#.to_string(),
            author: "author123".to_string(),
            committer: "committer123".to_string(),
            content: "content123".to_string(),
            created_at: NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap(), // 2021-01-01 00:00:00
        };
        
        // Convert to callisto model
        let callisto_model: crate::git_commit::Model = mercury_model.into();
        
        // Verify the conversion
        assert_eq!(callisto_model.id, 1);
        assert_eq!(callisto_model.repo_id, 123i64); // Should be converted from i32 to i64
        assert_eq!(callisto_model.commit_id, "commit123");
        assert_eq!(callisto_model.tree, "tree123");
        assert_eq!(callisto_model.author, "author123");
        assert_eq!(callisto_model.committer, "committer123");
        assert_eq!(callisto_model.content, "content123");
        assert_eq!(callisto_model.created_at, NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap());
        
        // Verify parents_id conversion
        let parents: Vec<String> = serde_json::from_value(callisto_model.parents_id).unwrap();
        assert_eq!(parents, vec!["parent1", "parent2"]);
    }
    
    #[test]
    fn test_convert_git_commit_active_model() {
        // Create a mercury GitCommitActiveModel
        let mercury_active_model = mercury::internal::model::sea_models::git_commit::ActiveModel {
            id: ActiveValue::Set(1),
            repo_id: ActiveValue::Set(123),
            commit_id: ActiveValue::Set("commit123".to_string()),
            tree: ActiveValue::Set("tree123".to_string()),
            parents_id: ActiveValue::Set(r#"["parent1", "parent2"]"#.to_string()),
            author: ActiveValue::Set("author123".to_string()),
            committer: ActiveValue::Set("committer123".to_string()),
            content: ActiveValue::Set("content123".to_string()),
            created_at: ActiveValue::Set(NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap()),
        };
        
        // Convert to callisto active model
        let callisto_active_model: crate::git_commit::ActiveModel = mercury_active_model.into();
        
        // Verify the conversion
        match callisto_active_model.id {
            ActiveValue::Set(val) => assert_eq!(val, 1),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.repo_id {
            ActiveValue::Set(val) => assert_eq!(val, 123i64), // Should be converted from i32 to i64
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.commit_id {
            ActiveValue::Set(ref val) => assert_eq!(val, "commit123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.tree {
            ActiveValue::Set(ref val) => assert_eq!(val, "tree123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.author {
            ActiveValue::Set(ref val) => assert_eq!(val, "author123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.committer {
            ActiveValue::Set(ref val) => assert_eq!(val, "committer123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.content {
            ActiveValue::Set(ref val) => assert_eq!(val, "content123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.created_at {
            ActiveValue::Set(val) => assert_eq!(val, NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap()),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        // Verify parents_id conversion
        match callisto_active_model.parents_id {
            ActiveValue::Set(ref val) => {
                let parents: Vec<String> = serde_json::from_str(val).unwrap();
                assert_eq!(parents, vec!["parent1", "parent2"]);
            },
            _ => panic!("Expected ActiveValue::Set"),
        }
    }
    
    #[test]
    fn test_convert_mega_commit_model() {
        // Create a mercury MegaCommitModel
        let mercury_model = mercury::internal::model::sea_models::mega_commit::Model {
            id: 1,
            commit_id: "commit123".to_string(),
            tree: "tree123".to_string(),
            parents_id: r#"["parent1", "parent2"]"#.to_string(),
            author: "author123".to_string(),
            committer: "committer123".to_string(),
            content: "content123".to_string(),
            created_at: NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap(), // 2021-01-01 00:00:00
        };
        
        // Convert to callisto model
        let callisto_model: crate::mega_commit::Model = mercury_model.into();
        
        // Verify the conversion
        assert_eq!(callisto_model.id, 1);
        assert_eq!(callisto_model.commit_id, "commit123");
        assert_eq!(callisto_model.tree, "tree123");
        assert_eq!(callisto_model.author, "author123");
        assert_eq!(callisto_model.committer, "committer123");
        assert_eq!(callisto_model.content, "content123");
        assert_eq!(callisto_model.created_at, NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap());
        
        // Verify parents_id conversion
        let parents: Vec<String> = serde_json::from_value(callisto_model.parents_id).unwrap();
        assert_eq!(parents, vec!["parent1", "parent2"]);
    }
    
    #[test]
    fn test_convert_mega_commit_active_model() {
        // Create a mercury MegaCommitActiveModel
        let mercury_active_model = mercury::internal::model::sea_models::mega_commit::ActiveModel {
            id: ActiveValue::Set(1),
            commit_id: ActiveValue::Set("commit123".to_string()),
            tree: ActiveValue::Set("tree123".to_string()),
            parents_id: ActiveValue::Set(r#"["parent1", "parent2"]"#.to_string()),
            author: ActiveValue::Set("author123".to_string()),
            committer: ActiveValue::Set("committer123".to_string()),
            content: ActiveValue::Set("content123".to_string()),
            created_at: ActiveValue::Set(NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap()),
        };
        
        // Convert to callisto active model
        let callisto_active_model: crate::mega_commit::ActiveModel = mercury_active_model.into();
        
        // Verify the conversion
        match callisto_active_model.id {
            ActiveValue::Set(val) => assert_eq!(val, 1),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.commit_id {
            ActiveValue::Set(ref val) => assert_eq!(val, "commit123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.tree {
            ActiveValue::Set(ref val) => assert_eq!(val, "tree123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.author {
            ActiveValue::Set(ref val) => assert_eq!(val, "author123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.committer {
            ActiveValue::Set(ref val) => assert_eq!(val, "committer123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.content {
            ActiveValue::Set(ref val) => assert_eq!(val, "content123"),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        match callisto_active_model.created_at {
            ActiveValue::Set(val) => assert_eq!(val, NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap()),
            _ => panic!("Expected ActiveValue::Set"),
        }
        
        // Verify parents_id conversion
        match callisto_active_model.parents_id {
            ActiveValue::Set(ref val) => {
                let parents: Vec<String> = serde_json::from_str(val).unwrap();
                assert_eq!(parents, vec!["parent1", "parent2"]);
            },
            _ => panic!("Expected ActiveValue::Set"),
        }
    }
    
    #[test]
    fn test_convert_with_empty_parents() {
        // Create a mercury GitCommitModel with empty parents
        let mercury_model = mercury::internal::model::sea_models::git_commit::Model {
            id: 1,
            repo_id: 123,
            commit_id: "commit123".to_string(),
            tree: "tree123".to_string(),
            parents_id: "[]".to_string(), // Empty array
            author: "author123".to_string(),
            committer: "committer123".to_string(),
            content: "content123".to_string(),
            created_at: NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap(),
        };
        
        // Convert to callisto model
        let callisto_model: crate::git_commit::Model = mercury_model.into();
        
        // Verify parents_id conversion
        let parents: Vec<String> = serde_json::from_value(callisto_model.parents_id).unwrap();
        assert_eq!(parents, Vec::<String>::new());
    }
    
    #[test]
    fn test_convert_with_invalid_json() {
        // Create a mercury GitCommitModel with invalid JSON
        let mercury_model = mercury::internal::model::sea_models::git_commit::Model {
            id: 1,
            repo_id: 123,
            commit_id: "commit123".to_string(),
            tree: "tree123".to_string(),
            parents_id: "invalid json".to_string(), // Invalid JSON
            author: "author123".to_string(),
            committer: "committer123".to_string(),
            content: "content123".to_string(),
            created_at: NaiveDateTime::from_timestamp_opt(1609459200, 0).unwrap(),
        };
        
        // Convert to callisto model - should not panic
        let callisto_model: crate::git_commit::Model = mercury_model.into();
        
        // Should default to empty array
        let parents: Vec<String> = serde_json::from_value(callisto_model.parents_id).unwrap();
        assert_eq!(parents, Vec::<String>::new());
    }
}