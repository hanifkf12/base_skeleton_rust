use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::user::{DisplayName, Email, User, UserId};

use super::{
    ApplicationError, CacheError, CreateUserInput, CreateUserUseCase, DeleteUserUseCase,
    GetUserUseCase, ListUsersInput, ListUsersUseCase, RepositoryError, UpdateUserInput,
    UpdateUserUseCase, UserCache, UserCreationJob, UserRegistrationRepository, UserRepository,
};

#[derive(Default)]
struct FakeUserRepository {
    users: Mutex<HashMap<UserId, User>>,
    jobs: Mutex<Vec<UserCreationJob>>,
}

#[async_trait]
impl UserRegistrationRepository for FakeUserRepository {
    async fn create_with_job(
        &self,
        user: &User,
        job: &UserCreationJob,
    ) -> Result<User, RepositoryError> {
        let created = self.create(user).await?;
        self.jobs.lock().unwrap().push(job.clone());
        Ok(created)
    }
}

#[async_trait]
impl UserRepository for FakeUserRepository {
    async fn create(&self, user: &User) -> Result<User, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        if users
            .values()
            .any(|existing| existing.email() == user.email())
        {
            return Err(RepositoryError::DuplicateEmail);
        }
        users.insert(user.id(), user.clone());
        Ok(user.clone())
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn list(&self, limit: u32, offset: u64) -> Result<Vec<User>, RepositoryError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .values()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn update(
        &self,
        user: &User,
        expected_updated_at: &DateTime<Utc>,
    ) -> Result<User, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        match users.get(&user.id()) {
            Some(existing) if existing.updated_at() == expected_updated_at => {
                users.insert(user.id(), user.clone());
                Ok(user.clone())
            }
            Some(_) => Err(RepositoryError::Conflict),
            None => Err(RepositoryError::NotFound),
        }
    }

    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError> {
        Ok(self.users.lock().unwrap().remove(&id).is_some())
    }
}

#[derive(Default)]
struct FakeUserCache {
    users: Mutex<HashMap<UserId, User>>,
}

#[async_trait]
impl UserCache for FakeUserCache {
    async fn get(&self, id: UserId) -> Result<Option<User>, CacheError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn set(&self, user: &User, _ttl_seconds: u64) -> Result<(), CacheError> {
        self.users.lock().unwrap().insert(user.id(), user.clone());
        Ok(())
    }

    async fn delete(&self, id: UserId) -> Result<(), CacheError> {
        self.users.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[tokio::test]
async fn create_user_validates_and_populates_cache() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let use_case = CreateUserUseCase::new(repository.clone(), cache.clone(), 60, 5);

    let user = use_case
        .execute(CreateUserInput {
            email: " ADA@EXAMPLE.COM ".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(user.email().as_str(), "ada@example.com");
    assert!(cache.get(user.id()).await.unwrap().is_some());
    let jobs = repository.jobs.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job_type, "user.created");
    assert_eq!(jobs[0].payload["user_id"], user.id().to_string());
}

#[tokio::test]
async fn get_user_falls_back_to_repository_and_warms_cache() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let user = User::new(
        Email::parse("grace@example.com").unwrap(),
        DisplayName::parse("Grace Hopper").unwrap(),
    );
    repository.create(&user).await.unwrap();
    let use_case = GetUserUseCase::new(repository, cache.clone(), 60);

    let found = use_case.execute(user.id()).await.unwrap();

    assert_eq!(found, user);
    assert_eq!(cache.get(user.id()).await.unwrap(), Some(user));
}

#[tokio::test]
async fn update_user_replaces_profile_and_warms_cache() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let user = User::new(
        Email::parse("old@example.com").unwrap(),
        DisplayName::parse("Old Name").unwrap(),
    );
    repository.create(&user).await.unwrap();
    let use_case = UpdateUserUseCase::new(repository.clone(), cache.clone(), 60);

    let updated = use_case
        .execute(
            user.id(),
            UpdateUserInput {
                email: "new@example.com".to_owned(),
                display_name: "New Name".to_owned(),
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.email().as_str(), "new@example.com");
    assert_eq!(updated.display_name().as_str(), "New Name");
    assert_eq!(cache.get(user.id()).await.unwrap(), Some(updated));
}

#[tokio::test]
async fn update_user_returns_not_found_for_missing_user() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let use_case = UpdateUserUseCase::new(repository, cache, 60);

    let result = use_case
        .execute(
            UserId::new(),
            UpdateUserInput {
                email: "x@example.com".to_owned(),
                display_name: "Some Name".to_owned(),
            },
        )
        .await;

    assert!(matches!(result, Err(ApplicationError::NotFound)));
}

#[tokio::test]
async fn update_user_returns_conflict_on_concurrent_modification() {
    let repository = Arc::new(StaleReadRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let user = User::new(
        Email::parse("ada@example.com").unwrap(),
        DisplayName::parse("Ada Lovelace").unwrap(),
    );
    repository.seed(user.clone());
    let use_case = UpdateUserUseCase::new(repository, cache, 60);

    let result = use_case
        .execute(
            user.id(),
            UpdateUserInput {
                email: "new@example.com".to_owned(),
                display_name: "New Name".to_owned(),
            },
        )
        .await;

    assert!(matches!(result, Err(ApplicationError::Conflict)));
}

#[tokio::test]
async fn delete_user_removes_user_and_cache_entry() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let user = User::new(
        Email::parse("grace@example.com").unwrap(),
        DisplayName::parse("Grace Hopper").unwrap(),
    );
    repository.create(&user).await.unwrap();
    cache.set(&user, 60).await.unwrap();
    let use_case = DeleteUserUseCase::new(repository.clone(), cache.clone());

    use_case.execute(user.id()).await.unwrap();

    assert!(repository.find_by_id(user.id()).await.unwrap().is_none());
    assert!(cache.get(user.id()).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_user_returns_not_found_for_missing_user() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let use_case = DeleteUserUseCase::new(repository, cache);

    let result = use_case.execute(UserId::new()).await;

    assert!(matches!(result, Err(ApplicationError::NotFound)));
}

#[tokio::test]
async fn list_users_returns_paginated_results() {
    let repository = Arc::new(FakeUserRepository::default());
    for i in 0..5 {
        let user = User::new(
            Email::parse(format!("user{i}@example.com")).unwrap(),
            DisplayName::parse(format!("User {i}")).unwrap(),
        );
        repository.create(&user).await.unwrap();
    }
    let use_case = ListUsersUseCase::new(repository);

    let page = use_case
        .execute(ListUsersInput {
            page: 1,
            per_page: 3,
        })
        .await
        .unwrap();

    assert_eq!(page.len(), 3);
}

#[tokio::test]
async fn create_user_rejects_duplicate_email() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FakeUserCache::default());
    let use_case = CreateUserUseCase::new(repository.clone(), cache.clone(), 60, 5);

    use_case
        .execute(CreateUserInput {
            email: "ada@example.com".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
        })
        .await
        .unwrap();

    let result = use_case
        .execute(CreateUserInput {
            email: "ada@example.com".to_owned(),
            display_name: "Ada Duplicate".to_owned(),
        })
        .await;

    assert!(matches!(result, Err(ApplicationError::EmailAlreadyExists)));
}

#[tokio::test]
async fn update_user_invalidates_cache_when_set_fails() {
    let repository = Arc::new(FakeUserRepository::default());
    let cache = Arc::new(FailingSetCache::default());
    let user = User::new(
        Email::parse("ada@example.com").unwrap(),
        DisplayName::parse("Ada Lovelace").unwrap(),
    );
    repository.create(&user).await.unwrap();
    cache.prime(&user).await;
    let use_case = UpdateUserUseCase::new(repository, cache.clone(), 60);

    let updated = use_case
        .execute(
            user.id(),
            UpdateUserInput {
                email: "new@example.com".to_owned(),
                display_name: "New Name".to_owned(),
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.email().as_str(), "new@example.com");
    assert!(cache.get(user.id()).await.unwrap().is_none());
}

#[derive(Default)]
struct FailingSetCache {
    users: Mutex<HashMap<UserId, User>>,
}

impl FailingSetCache {
    async fn prime(&self, user: &User) {
        self.users.lock().unwrap().insert(user.id(), user.clone());
    }
}

#[async_trait]
impl UserCache for FailingSetCache {
    async fn get(&self, id: UserId) -> Result<Option<User>, CacheError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn set(&self, _user: &User, _ttl_seconds: u64) -> Result<(), CacheError> {
        Err(CacheError::Unavailable("simulated set failure".into()))
    }

    async fn delete(&self, id: UserId) -> Result<(), CacheError> {
        self.users.lock().unwrap().remove(&id);
        Ok(())
    }
}

struct StaleReadRepository {
    stale: Mutex<Option<User>>,
    stored: Mutex<Option<User>>,
}

impl Default for StaleReadRepository {
    fn default() -> Self {
        Self {
            stale: Mutex::new(None),
            stored: Mutex::new(None),
        }
    }
}

impl StaleReadRepository {
    fn seed(&self, user: User) {
        let mut modified = user.clone();
        modified.update_profile(
            Email::parse("concurrent@example.com").unwrap(),
            DisplayName::parse("Concurrent Edit").unwrap(),
        );
        *self.stale.lock().unwrap() = Some(user);
        *self.stored.lock().unwrap() = Some(modified);
    }
}

#[async_trait]
impl UserRepository for StaleReadRepository {
    async fn create(&self, _user: &User) -> Result<User, RepositoryError> {
        unimplemented!()
    }

    async fn find_by_id(&self, _id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(self.stale.lock().unwrap().clone())
    }

    async fn list(&self, _limit: u32, _offset: u64) -> Result<Vec<User>, RepositoryError> {
        unimplemented!()
    }

    async fn update(
        &self,
        user: &User,
        expected_updated_at: &DateTime<Utc>,
    ) -> Result<User, RepositoryError> {
        let stored = self.stored.lock().unwrap();
        match stored.as_ref() {
            Some(existing) if existing.updated_at() == expected_updated_at => Ok(user.clone()),
            Some(_) => Err(RepositoryError::Conflict),
            None => Err(RepositoryError::NotFound),
        }
    }

    async fn delete(&self, _id: UserId) -> Result<bool, RepositoryError> {
        unimplemented!()
    }
}

#[async_trait]
impl UserRegistrationRepository for StaleReadRepository {
    async fn create_with_job(
        &self,
        _user: &User,
        _job: &UserCreationJob,
    ) -> Result<User, RepositoryError> {
        unimplemented!()
    }
}
