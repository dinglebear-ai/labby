//! Request bounds constrain upstream work, not just the returned vector.

use super::*;

fn preview(max_items: usize) -> SkillDiscoverRequest {
    SkillDiscoverRequest {
        max_items,
        deadline: SkillProviderDeadline::default(),
    }
}

#[tokio::test]
async fn bounded_discovery_stops_before_following_another_page() {
    let server = SkillsServer::new(vec![
        json!({"skills": [entry("up", "alpha")], "nextCursor": "second"}),
        json!({"skills": [entry("up", "beta")]}),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    let result = provider.discover(&preview(1)).await.unwrap();
    assert_eq!(result.skills.len(), 1);
    assert!(result.truncated);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "do not fetch a page that cannot contribute"
    );
    assert!(
        pool.skills_cache.read().await.is_empty(),
        "a preview must not masquerade as a full cached listing"
    );
}

#[tokio::test]
async fn bounded_discovery_limits_invalid_candidate_work() {
    let invalid = (0..40)
        .map(|i| {
            let mut candidate = entry("up", &format!("bad-{i}"));
            candidate["frontmatter"]["name"] = json!("INVALID NAME");
            candidate
        })
        .collect::<Vec<_>>();
    let server = SkillsServer::new(vec![
        json!({"skills": invalid, "nextCursor": "more"}),
        json!({"skills": [entry("up", "valid")]}),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let provider = super::super::SepSkillProvider::new(pool, skills_config("up", None), None);
    let result = provider.discover(&preview(1)).await.unwrap();
    assert!(result.skills.is_empty());
    assert!(result.truncated);
    assert_eq!(
        result.excluded_count, 4,
        "invalid entries consume the proportional candidate budget"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bounded_discovery_preserves_a_larger_cached_catalog() {
    let server = SkillsServer::new(vec![json!({
        "skills": [entry("up", "alpha"), entry("up", "beta"), entry("up", "gamma")]
    })]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    assert_eq!(
        provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .len(),
        3
    );
    let result = provider.discover(&preview(1)).await.unwrap();
    assert_eq!(result.skills.len(), 1);
    assert!(result.truncated);
    assert_eq!(
        result.source,
        labby_runtime::skills::SkillDiscoverySource::Cached
    );
    assert_eq!(
        provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .len(),
        3
    );
    let narrowed =
        super::super::SepSkillProvider::new(pool, skills_config("up", Some(vec!["beta"])), None);
    let filtered = narrowed.discover(&preview(1)).await.unwrap();
    assert_eq!(filtered.skills.len(), 1);
    assert_eq!(filtered.skills[0].descriptor().name, "beta");
    assert!(
        !filtered.truncated,
        "an exact visible match is not budget truncation"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a warm preview performs no upstream listing"
    );
}

#[tokio::test]
async fn bounded_discovery_cold_preview_preserves_full_list_and_direct_get() {
    let server = SkillsServer::new(vec![json!({
        "skills": [entry("up", "alpha"), entry("up", "beta"), entry("up", "gamma")]
    })])
    .with_get(entry("up", "unlisted"));
    let calls = Arc::clone(&server.list_calls);
    let gets = Arc::clone(&server.get_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    assert!(provider.discover(&preview(1)).await.unwrap().truncated);
    assert!(pool.skills_cache.read().await.is_empty());
    let id = provider_skill_id("up", "unlisted");
    let found = provider
        .get(&SkillGetRequest {
            id: id.clone(),
            deadline: SkillProviderDeadline::default(),
        })
        .await
        .unwrap();
    assert_eq!(found.skill.descriptor().name, "unlisted");
    let file = provider
        .read_resource(&SkillResourceReadRequest {
            skill_id: id,
            resource_id: "skill://up/unlisted/SKILL.md".to_string(),
            max_bytes: limits::MAX_SKILL_RESOURCE_BYTES,
            deadline: SkillProviderDeadline::default(),
        })
        .await
        .unwrap();
    assert_eq!(file.bytes, skill_md_body("unlisted").as_bytes());
    assert_eq!(
        provider
            .discover(&SkillDiscoverRequest::default())
            .await
            .unwrap()
            .skills
            .len(),
        3
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the later full operation must perform a complete listing"
    );
    assert_eq!(gets.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bounded_discovery_never_reuses_another_oauth_subjects_cache() {
    let server = SkillsServer::new(vec![
        json!({"skills": [entry("up", "alice-only")]}),
        json!({"skills": [entry("up", "bob-only")]}),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let alice = super::super::SepSkillProvider::new(
        Arc::clone(&pool),
        oauth_skills_config("up", None),
        Some("alice".to_string()),
    );
    let bob = super::super::SepSkillProvider::new(
        Arc::clone(&pool),
        oauth_skills_config("up", None),
        Some("bob".to_string()),
    );
    alice
        .discover(&SkillDiscoverRequest::default())
        .await
        .unwrap();
    let result = bob.discover(&preview(1)).await.unwrap();
    assert_eq!(result.skills[0].descriptor().name, "bob-only");
    let result = alice.discover(&preview(1)).await.unwrap();
    assert_eq!(result.skills[0].descriptor().name, "alice-only");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bounded_discovery_honors_proxy_opt_in() {
    let server = SkillsServer::new(vec![json!({"skills": [entry("up", "alpha") ]})]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let mut config = skills_config("up", None);
    config.proxy_skills = false;
    let provider = super::super::SepSkillProvider::new(pool, config, None);
    let result = provider.discover(&preview(1)).await.unwrap();
    assert!(result.skills.is_empty());
    assert!(!result.truncated);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bounded_discovery_rejects_an_invalidated_in_flight_preview() {
    #[derive(Clone)]
    struct GatedServer {
        inner: SkillsServer,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    impl ServerHandler for GatedServer {
        fn get_info(&self) -> ServerInfo {
            self.inner.get_info()
        }
        async fn on_custom_request(
            &self,
            request: CustomRequest,
            context: RequestContext<RoleServer>,
        ) -> Result<CustomResult, ErrorData> {
            if request.method.as_str() == "skills/list" {
                self.started.notify_one();
                self.release.notified().await;
            }
            self.inner.on_custom_request(request, context).await
        }
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let server = GatedServer {
        inner: SkillsServer::new(vec![json!({"skills": [entry("up", "alpha")]})]),
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    };
    let pool = catalog_pool_with_server("up", server).await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    let task = tokio::spawn(async move { provider.discover(&preview(1)).await });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    pool.invalidate_upstream_skills("up").await;
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(SkillProviderError::Unavailable { .. })),
        "a pre-invalidation response cannot be returned as current discovery"
    );
    assert!(pool.skills_cache.read().await.is_empty());
}

#[tokio::test]
async fn bounded_discovery_refreshes_expired_data_without_replacing_full_snapshot() {
    let server = SkillsServer::new(vec![
        json!({"skills": [entry("up", "old-alpha"), entry("up", "old-beta")]}),
        json!({"skills": [entry("up", "fresh-alpha")]}),
    ]);
    let calls = Arc::clone(&server.list_calls);
    let pool = catalog_pool_with_server("up", server).await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    provider
        .discover(&SkillDiscoverRequest::default())
        .await
        .unwrap();
    let key = ("up".to_string(), None);
    pool.skills_cache
        .write()
        .await
        .get_mut(&key)
        .unwrap()
        .expire_now();
    let result = provider.discover(&preview(1)).await.unwrap();
    assert_eq!(result.skills[0].descriptor().name, "fresh-alpha");
    assert_eq!(
        result.source,
        labby_runtime::skills::SkillDiscoverySource::Refreshed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let cache = pool.skills_cache.read().await;
    let full = cache.get(&key).unwrap();
    assert_eq!(
        full.skills.skills.len(),
        2,
        "the preview must not replace the operator snapshot"
    );
    assert!(
        !full.is_fresh(),
        "preview freshness must not be attributed to the full catalog"
    );
    assert!(
        !full.refreshing,
        "a bounded read must not spawn a full catalog walk"
    );
}
