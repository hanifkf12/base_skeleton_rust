use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::domain::user::{DisplayName, Email, User, UserId};

use super::{
    CacheError, CreateUserInput, CreateUserUseCase, GetUserUseCase, RepositoryError, UserCache,
    UserRepository,
};

#[derive(Default)]
struct FakeUserRepository {
    users: Mutex<HashMap<UserId, User>>,
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

    async fn update(&self, user: &User) -> Result<Option<User>, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        if !users.contains_key(&user.id()) {
            return Ok(None);
        }
        users.insert(user.id(), user.clone());
        Ok(Some(user.clone()))
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
    let use_case = CreateUserUseCase::new(repository, cache.clone(), 60);

    let user = use_case
        .execute(CreateUserInput {
            email: " ADA@EXAMPLE.COM ".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
        })
        .await
        .unwrap();

    assert_eq!(user.email().as_str(), "ada@example.com");
    assert!(cache.get(user.id()).await.unwrap().is_some());
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
