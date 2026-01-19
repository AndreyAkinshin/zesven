//! Archive modification operations.

use crate::ArchivePath;

/// A pending modification operation on an archive.
#[derive(Debug, Clone)]
pub enum Operation {
    /// Rename an entry in the archive.
    Rename {
        /// Original path.
        from: ArchivePath,
        /// New path.
        to: ArchivePath,
    },
    /// Delete an entry from the archive.
    Delete {
        /// Path to delete.
        path: ArchivePath,
    },
    /// Update an existing entry with new data.
    Update {
        /// Path to update.
        path: ArchivePath,
        /// New data (will be compressed).
        data: Vec<u8>,
    },
    /// Add a new entry to the archive.
    Add {
        /// Path for new entry.
        path: ArchivePath,
        /// Data for new entry (will be compressed).
        data: Vec<u8>,
    },
}

impl Operation {
    /// Returns the target path of this operation.
    pub fn target_path(&self) -> &ArchivePath {
        match self {
            Operation::Rename { to, .. } => to,
            Operation::Delete { path } => path,
            Operation::Update { path, .. } => path,
            Operation::Add { path, .. } => path,
        }
    }

    /// Returns the source path for this operation (for rename/delete/update).
    pub fn source_path(&self) -> Option<&ArchivePath> {
        match self {
            Operation::Rename { from, .. } => Some(from),
            Operation::Delete { path } => Some(path),
            Operation::Update { path, .. } => Some(path),
            Operation::Add { .. } => None,
        }
    }

    /// Returns whether this is a header-only change (no data recompression needed).
    pub fn is_header_only(&self) -> bool {
        matches!(self, Operation::Rename { .. } | Operation::Delete { .. })
    }

    /// Returns the operation type as a string.
    pub fn operation_type(&self) -> &'static str {
        match self {
            Operation::Rename { .. } => "rename",
            Operation::Delete { .. } => "delete",
            Operation::Update { .. } => "update",
            Operation::Add { .. } => "add",
        }
    }
}

/// Builder for creating operations fluently.
#[derive(Debug, Default)]
pub struct OperationBuilder {
    operations: Vec<Operation>,
}

impl OperationBuilder {
    /// Creates a new operation builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a rename operation.
    pub fn rename(mut self, from: ArchivePath, to: ArchivePath) -> Self {
        self.operations.push(Operation::Rename { from, to });
        self
    }

    /// Adds a delete operation.
    pub fn delete(mut self, path: ArchivePath) -> Self {
        self.operations.push(Operation::Delete { path });
        self
    }

    /// Adds an update operation.
    pub fn update(mut self, path: ArchivePath, data: impl Into<Vec<u8>>) -> Self {
        self.operations.push(Operation::Update {
            path,
            data: data.into(),
        });
        self
    }

    /// Adds an add operation.
    pub fn add(mut self, path: ArchivePath, data: impl Into<Vec<u8>>) -> Self {
        self.operations.push(Operation::Add {
            path,
            data: data.into(),
        });
        self
    }

    /// Builds the list of operations.
    pub fn build(self) -> Vec<Operation> {
        self.operations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_target_path() {
        let path = ArchivePath::new("test.txt").unwrap();
        let op = Operation::Delete { path: path.clone() };
        assert_eq!(op.target_path(), &path);
    }

    #[test]
    fn test_operation_source_path() {
        let from = ArchivePath::new("old.txt").unwrap();
        let to = ArchivePath::new("new.txt").unwrap();
        let op = Operation::Rename {
            from: from.clone(),
            to,
        };
        assert_eq!(op.source_path(), Some(&from));

        let add_op = Operation::Add {
            path: ArchivePath::new("new.txt").unwrap(),
            data: vec![],
        };
        assert!(add_op.source_path().is_none());
    }

    #[test]
    fn test_operation_is_header_only() {
        let rename_op = Operation::Rename {
            from: ArchivePath::new("a").unwrap(),
            to: ArchivePath::new("b").unwrap(),
        };
        assert!(rename_op.is_header_only());

        let delete_op = Operation::Delete {
            path: ArchivePath::new("a").unwrap(),
        };
        assert!(delete_op.is_header_only());

        let update_op = Operation::Update {
            path: ArchivePath::new("a").unwrap(),
            data: vec![1, 2, 3],
        };
        assert!(!update_op.is_header_only());
    }

    #[test]
    fn test_operation_builder() {
        let ops = OperationBuilder::new()
            .rename(
                ArchivePath::new("a.txt").unwrap(),
                ArchivePath::new("b.txt").unwrap(),
            )
            .delete(ArchivePath::new("c.txt").unwrap())
            .add(ArchivePath::new("d.txt").unwrap(), b"data".to_vec())
            .build();

        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].operation_type(), "rename");
        assert_eq!(ops[1].operation_type(), "delete");
        assert_eq!(ops[2].operation_type(), "add");
    }
}
