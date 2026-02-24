use git_internal::internal::object::intent::Intent;

use crate::utils::storage_ext::Identifiable;

impl Identifiable for Intent {
    fn object_id(&self) -> String {
        self.header().object_id().to_string()
    }

    fn object_type(&self) -> String {
        self.header().object_type().to_string()
    }
}
