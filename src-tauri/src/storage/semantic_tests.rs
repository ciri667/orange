//! 固定向量验证权限、过滤和任务失效，不依赖真实 embedding 服务。
use super::*;
use crate::domain::SemanticSearchSettings;
use crate::retrieval::provider;

fn database() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("semantic.sqlite");
    let connection = Connection::open(&path).unwrap();
    ensure_database_schema(&connection, &path).unwrap();
    (dir, connection)
}

fn settings() -> UserSettings {
    let mut settings = default_user_settings();
    settings.semantic_search = SemanticSearchSettings {
        enabled: true,
        model: "fixture".into(),
        ..Default::default()
    };
    settings
}

fn seed(connection: &Connection) {
    connection.execute_batch("INSERT INTO knowledge_bases VALUES ('a','a','/redacted',1,'now');
        INSERT INTO semantic_index_state(knowledge_base_id,enabled) VALUES ('a',1);
        INSERT INTO notes(id,knowledge_base_id,title,path,content_hash,updated_at,file_type,updated_at_unix) VALUES ('n','a','队列','docs/a.md','h','2026-10-01','markdown',1790812800);
        INSERT INTO note_chunks VALUES ('n:0','n','a',0,'队列','消费失败后重试',2,3,'h');
        INSERT INTO semantic_inputs VALUES ('n:0','a','input-hash','固定测试文本');
        INSERT INTO note_tags VALUES ('n','a','架构');").unwrap();
}

fn job(settings: &UserSettings, generation: i64) -> SemanticJob {
    SemanticJob {
        chunk_id: "n:0".into(),
        knowledge_base_id: "a".into(),
        input_hash: "input-hash".into(),
        input: "固定测试文本".into(),
        fingerprint: provider::fingerprint(&settings.semantic_search),
        generation,
        attempts: 0,
    }
}

#[test]
fn schema_is_idempotent_and_preserves_legacy_embeddings() {
    let (_dir, c) = database();
    c.execute(
        "INSERT INTO embeddings VALUES('legacy',0,?1,'old','now')",
        [vec![1_u8]],
    )
    .unwrap();
    ensure_semantic_schema(&c).unwrap();
    ensure_semantic_schema(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM embeddings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn queue_deduplicates_and_reuses_input_hash_after_chunk_moves() {
    let (_dir, c) = database();
    seed(&c);
    let s = settings();
    reconcile_semantic(&c, &s).unwrap();
    reconcile_semantic(&c, &s).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM semantic_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(complete_semantic_job(&c, &s, &job(&s, 1), &[1., 0.]).unwrap());
    c.execute(
        "UPDATE semantic_inputs SET chunk_id='n:1' WHERE chunk_id='n:0'",
        [],
    )
    .unwrap();
    reconcile_semantic(&c, &s).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM semantic_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn in_flight_response_cannot_restore_changed_deleted_or_revoked_input() {
    for change in [
        "UPDATE semantic_inputs SET input_hash='new'",
        "DELETE FROM note_chunks",
        "UPDATE semantic_index_state SET enabled=0",
        "UPDATE semantic_index_state SET generation=generation+1",
        "DELETE FROM knowledge_bases",
        "UPDATE semantic_index_state SET paused=1",
    ] {
        let (_dir, c) = database();
        seed(&c);
        let s = settings();
        reconcile_semantic(&c, &s).unwrap();
        c.execute_batch(change).unwrap();
        assert!(
            !complete_semantic_job(&c, &s, &job(&s, 1), &[1., 0.]).unwrap(),
            "{change}"
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM semantic_vectors", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn changed_model_never_accepts_previous_response() {
    let (_dir, c) = database();
    seed(&c);
    let mut s = settings();
    reconcile_semantic(&c, &s).unwrap();
    let old = job(&s, 1);
    s.semantic_search.model = "new-model".into();
    assert!(!complete_semantic_job(&c, &s, &old, &[1., 0.]).unwrap());
    reconcile_semantic(&c, &s).unwrap();
    assert_eq!(semantic_statuses(&c, &s).unwrap()[0].indexed_chunks, 0);
}

#[test]
fn cloud_consent_is_bound_to_endpoint_model_and_privacy() {
    let mut s = settings();
    s.semantic_search.provider = "openai-compatible".into();
    s.semantic_search.api_base = "https://embedding.example/v1".into();
    assert!(semantic_allowed(&s).is_err());
    s.semantic_search.cloud_consent_fingerprint = Some(provider::fingerprint(&s.semantic_search));
    assert!(semantic_allowed(&s).is_ok());
    s.privacy_policy = "local-only".into();
    assert!(semantic_allowed(&s).is_err());
    s.privacy_policy = "allow-selected-scope".into();
    s.semantic_search.model = "changed".into();
    assert!(semantic_allowed(&s).is_err());
}

#[test]
fn pause_resume_rebuild_disable_keep_consistent_queue_and_cache() {
    let (_dir,c)=database();seed(&c);let s=settings();reconcile_semantic(&c,&s).unwrap();
    complete_semantic_job(&c,&s,&job(&s,1),&[1.,0.]).unwrap();
    let act=|action:&str| apply_semantic_action_in_transaction(&c,&s,&crate::domain::SemanticIndexAction{knowledge_base_id:"a".into(),action:action.into(),allow_cloud_upload:false}).unwrap();
    let paused=act("pause"); assert!(paused[0].paused);assert_eq!(paused[0].indexed_chunks,1);
    let resumed=act("resume");assert!(!resumed[0].paused);assert_eq!(resumed[0].indexed_chunks,1);
    let rebuilt=act("rebuild");assert_eq!(rebuilt[0].indexed_chunks,0);assert_eq!(rebuilt[0].pending_chunks,1);
    assert!(!complete_semantic_job(&c,&s,&job(&s,1),&[1.,0.]).unwrap());
    let disabled=act("disable");assert!(!disabled[0].enabled);
    assert_eq!(c.query_row("SELECT count(*) FROM semantic_jobs",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT count(*) FROM semantic_vectors",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    let enabled=act("enable");assert!(enabled[0].enabled);assert_eq!(enabled[0].pending_chunks,1);
}

#[test]
fn switching_back_to_cached_model_recovers_dimension_without_network() {
    let (_dir,c)=database();seed(&c);let mut s=settings();reconcile_semantic(&c,&s).unwrap();
    complete_semantic_job(&c,&s,&job(&s,1),&[1.,0.]).unwrap();
    s.semantic_search.model="other".into();reconcile_semantic(&c,&s).unwrap();
    assert_eq!(semantic_statuses(&c,&s).unwrap()[0].indexed_chunks,0);
    s.semantic_search.model="fixture".into();reconcile_semantic(&c,&s).unwrap();
    let status=semantic_statuses(&c,&s).unwrap();assert_eq!(status[0].indexed_chunks,1);assert_eq!(status[0].dimensions,Some(2));
}

#[test]
fn restart_recovers_persisted_pending_jobs() {
    let (dir,c)=database();seed(&c);let s=settings();reconcile_semantic(&c,&s).unwrap();drop(c);
    let c=Connection::open(dir.path().join("semantic.sqlite")).unwrap();
    assert!(complete_semantic_job(&c,&s,&job(&s,1),&[1.,0.]).unwrap());
    assert_eq!(semantic_statuses(&c,&s).unwrap()[0].indexed_chunks,1);
}
