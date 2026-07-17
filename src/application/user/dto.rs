#[derive(Debug, Clone)]
pub struct CreateUserInput {
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct UpdateUserInput {
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ListUsersInput {
    pub page: u32,
    pub per_page: u32,
}

impl ListUsersInput {
    pub fn normalized(self) -> Self {
        Self {
            page: self.page.max(1),
            per_page: self.per_page.clamp(1, 100),
        }
    }

    pub fn offset(self) -> u64 {
        let normalized = self.normalized();
        u64::from(normalized.page - 1) * u64::from(normalized.per_page)
    }
}
