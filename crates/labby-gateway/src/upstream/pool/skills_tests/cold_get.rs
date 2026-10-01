use super::*;

#[derive(Clone)]
struct GetReleasesList {
    inner: SkillsServer,
    get_started: Arc<tokio::sync::Notify>,
}
impl ServerHandler for GetReleasesList {
    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        match request.method.as_str() {
            "skills/list" => self.get_started.notified().await,
            "skills/get" => self.get_started.notify_one(),
            _ => {}
        }
        self.inner.on_custom_request(request, context).await
    }
}

#[tokio::test]
async fn cold_targeted_get_overlaps_catalog_validation() {
    let server = GetReleasesList {
        inner: SkillsServer::new(vec![json!({"skills": []})]).with_get(entry("up", "unlisted")),
        get_started: Arc::new(tokio::sync::Notify::new()),
    };
    let pool = catalog_pool_with_server("up", server).await;
    let provider = super::super::SepSkillProvider::new(pool, skills_config("up", None), None);
    let result = provider
        .get(&SkillGetRequest {
            id: provider_skill_id("up", "unlisted"),
            deadline: SkillProviderDeadline {
                timeout: Duration::from_millis(250),
            },
        })
        .await
        .expect("cold get dispatches while ownership catalog is pending");
    assert_eq!(result.skill.descriptor().name, "unlisted");
}

#[tokio::test]
async fn cold_listed_get_does_not_wait_for_speculative_target() {
    let server = SkillsServer::new(vec![json!({"skills": [entry("up", "listed")]})]).stalling_get();
    let pool = catalog_pool_with_server("up", server).await;
    let provider = super::super::SepSkillProvider::new(pool, skills_config("up", None), None);
    let result = provider
        .get(&SkillGetRequest {
            id: provider_skill_id("up", "listed"),
            deadline: SkillProviderDeadline {
                timeout: Duration::from_millis(250),
            },
        })
        .await
        .expect("authoritative listed entry wins over a stalled speculative get");
    assert_eq!(result.skill.descriptor().name, "listed");
}

#[derive(Clone)]
struct GatedGet {
    inner: SkillsServer,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl ServerHandler for GatedGet {
    fn get_info(&self) -> ServerInfo {
        self.inner.get_info()
    }
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        if request.method.as_str() == "skills/get" {
            self.started.notify_one();
            self.release.notified().await;
        }
        self.inner.on_custom_request(request, context).await
    }
}

#[tokio::test]
async fn targeted_get_cannot_publish_into_a_replacement_catalog() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let pool = catalog_pool_with_server(
        "up",
        GatedGet {
            inner: SkillsServer::new(vec![json!({"skills": []})]).with_get(entry("up", "unlisted")),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        },
    )
    .await;
    let config = skills_config("up", None);
    pool.upstream_skills(&config, None)
        .await
        .expect("initial catalog");
    let provider = super::super::SepSkillProvider::new(Arc::clone(&pool), config.clone(), None);
    let task = tokio::spawn(async move {
        provider
            .get(&SkillGetRequest {
                id: provider_skill_id("up", "unlisted"),
                deadline: SkillProviderDeadline::default(),
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("get started");
    pool.invalidate_upstream_skills("up").await;
    pool.upstream_skills(&config, None)
        .await
        .expect("replacement catalog");
    release.notify_one();
    assert!(
        task.await.expect("get task").is_err(),
        "late response must be rejected"
    );
    assert!(
        pool.cached_direct_skill(&config, None, "skill://up/unlisted/SKILL.md")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn cold_listed_get_preserves_single_permit_catalog_progress() {
    let server = SkillsServer::new(vec![json!({"skills": [entry("up", "listed")]})]).stalling_get();
    let pool = catalog_pool_with_server("up", server).await;
    let pool = Arc::new(
        Arc::try_unwrap(pool)
            .ok()
            .unwrap()
            .with_upstream_call_concurrency(1),
    );
    // A concurrent catalog request may hold the acquisition lock. Speculative
    // get must not consume the only RPC permit while authoritative list waits.
    let guard = pool
        .skills_fetch_locks
        .guard_for(&("up".to_string(), None))
        .await;
    let locked = guard.lock().await;
    let provider =
        super::super::SepSkillProvider::new(Arc::clone(&pool), skills_config("up", None), None);
    let task = tokio::spawn(async move {
        provider
            .get(&SkillGetRequest {
                id: provider_skill_id("up", "listed"),
                deadline: SkillProviderDeadline {
                    timeout: Duration::from_millis(250),
                },
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    drop(locked);
    let result = task
        .await
        .expect("get task")
        .expect("listed catalog retains sole permit");
    assert_eq!(result.skill.descriptor().name, "listed");
}

#[tokio::test]
async fn cold_catalog_revision_probe_never_waits_for_upstream_listing() {
    let server = SkillsServer::new(vec![json!({"skills": []})]).stalling_list();
    let pool = catalog_pool_with_server("up", server).await;
    let provider = super::super::SepSkillProvider::new(pool, skills_config("up", None), None);
    let _revision = tokio::time::timeout(Duration::from_millis(25), provider.catalog_revision())
        .await
        .expect("cold cache probe returns immediately without upstream I/O");
}
