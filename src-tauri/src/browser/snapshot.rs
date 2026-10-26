//! 给模型的快照投影：正文和元素分别返回，元素分页保留完整 reference 与续读位置。

use super::types::BrowserSnapshot;

/** 每页最多 20 个元素，避免整个 DOM 元素集挤占模型上下文。 */
const MODEL_ELEMENTS_PER_PAGE: usize = 20;

/** 快照已按正文字符数和元素字段长度限量，不再使用通用的字符串摘要截断。 */
pub(super) fn project_model_snapshot(
    mut snapshot: BrowserSnapshot,
    operation: &str,
    element_offset: usize,
) -> BrowserSnapshot {
    if operation == "page.text.snapshot" {
        snapshot.elements.clear();
        snapshot.next_element_offset = None;
    } else {
        snapshot.blocks.clear();
        snapshot.next_text_offset = None;
        let total = snapshot.elements.len();
        snapshot.elements = snapshot
            .elements
            .into_iter()
            .skip(element_offset)
            .take(MODEL_ELEMENTS_PER_PAGE)
            .collect();
        let end = element_offset.saturating_add(snapshot.elements.len());
        snapshot.next_element_offset = (end < total).then_some(end);
        snapshot.truncated = snapshot.next_element_offset.is_some();
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserElement, BrowserRect, BrowserTextBlock};

    fn snapshot_fixture() -> BrowserSnapshot {
        BrowserSnapshot {
            snapshot_id: "snap-test".to_owned(),
            page_generation: 3,
            captured_at: 1,
            blocks: vec![BrowserTextBlock {
                id: "body".to_owned(),
                text: "中文正文".repeat(1500),
            }],
            elements: (0..45)
                .map(|index| BrowserElement {
                    snapshot_id: "snap-test".to_owned(),
                    reference: format!("snap-test-{index}"),
                    role: "input".to_owned(),
                    name: "字段名称".repeat(60),
                    text: Some("元素文本".repeat(60)),
                    value: Some(format!("字段值 {index}")),
                    state: None,
                    rect: BrowserRect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 20.0,
                    },
                })
                .collect(),
            truncated: true,
            next_text_offset: Some(6000),
            next_element_offset: None,
        }
    }

    #[test]
    fn text_snapshot_keeps_body_and_continuation_without_repeating_elements() {
        let snapshot = project_model_snapshot(snapshot_fixture(), "page.text.snapshot", 0);
        assert_eq!(snapshot.blocks[0].text.chars().count(), 6000);
        assert_eq!(snapshot.next_text_offset, Some(6000));
        assert!(snapshot.truncated);
        assert!(snapshot.elements.is_empty());
        assert!(snapshot.next_element_offset.is_none());
    }

    #[test]
    fn element_pages_preserve_all_references_and_report_the_end() {
        let mut references = Vec::new();
        let mut offset = 0;
        loop {
            let snapshot =
                project_model_snapshot(snapshot_fixture(), "page.elements.snapshot", offset);
            assert!(snapshot.blocks.is_empty());
            assert!(snapshot.next_text_offset.is_none());
            assert_eq!(snapshot.page_generation, 3);
            references.extend(
                snapshot
                    .elements
                    .iter()
                    .map(|element| element.reference.clone()),
            );
            let Some(next) = snapshot.next_element_offset else {
                assert!(!snapshot.truncated);
                break;
            };
            assert!(snapshot.truncated);
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(
            references,
            (0..45)
                .map(|index| format!("snap-test-{index}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn element_offset_past_the_end_returns_an_empty_finished_page() {
        let snapshot =
            project_model_snapshot(snapshot_fixture(), "page.elements.snapshot", usize::MAX);
        assert!(snapshot.elements.is_empty());
        assert!(!snapshot.truncated);
        assert!(snapshot.next_element_offset.is_none());
    }

    /** 用真实 Chromium 验证大页面分页后的 reference 仍能定位并输入，不依赖模型或外网。 */
    #[tokio::test]
    #[ignore = "requires ORANGE_BROWSER_TEST_EXECUTABLE pointing to an installed Chromium"]
    async fn live_browser_paged_snapshot_keeps_later_elements_actionable() {
        use super::super::{input, manager, session::BrowserSessionState, BrowserUserInput};
        use std::path::PathBuf;

        let executable = PathBuf::from(std::env::var("ORANGE_BROWSER_TEST_EXECUTABLE").unwrap());
        let directory = tempfile::tempdir().unwrap();
        let mut session = BrowserSessionState::launch(executable, directory.path().join("profile"))
            .await
            .unwrap();
        // 启动只创建进程；测试页面与用户新建页签一样显式创建。
        session.create_tab().await.unwrap();
        let page = session.tabs[&session.active_tab_id].page.clone();
        let fields = (0..45)
            .map(|index| {
                format!(
                    "<input name='field-{index}' aria-label='{}' value='{}'>",
                    "字段名称".repeat(60),
                    "字段内容".repeat(60),
                )
            })
            .collect::<String>();
        let body = format!("<!doctype html><meta charset='utf-8'><body>{fields}</body>");
        // 此用例验证 DOM 分页和输入，直接装载固定 HTML，避免一次性 HTTP 服务的连接竞态。
        page.set_content(body).await.unwrap();

        let mut references = Vec::new();
        let mut offset = 0;
        loop {
            let snapshot = manager::capture_snapshot(&page, 0).await.unwrap();
            let projected = project_model_snapshot(snapshot, "page.elements.snapshot", offset);
            if offset == 0 {
                assert!(serde_json::to_string(&projected).unwrap().chars().count() > 9000);
            }
            references.extend(
                projected
                    .elements
                    .into_iter()
                    .map(|element| element.reference),
            );
            let Some(next) = projected.next_element_offset else {
                break;
            };
            offset = next;
        }
        assert_eq!(references.len(), 45);
        let target = page
            .find_element(format!("[data-orange-ref=\"{}\"]", references[44]))
            .await
            .unwrap();
        target.focus().await.unwrap();
        input::send_input(
            &page,
            BrowserUserInput::Key {
                key: "a".to_owned(),
                modifiers: 2,
            },
        )
        .await
        .unwrap();
        input::send_input(
            &page,
            BrowserUserInput::Text {
                text: "分页输入成功".to_owned(),
            },
        )
        .await
        .unwrap();
        let updated = project_model_snapshot(
            manager::capture_snapshot(&page, 0).await.unwrap(),
            "page.elements.snapshot",
            40,
        );
        assert_eq!(updated.elements[4].value.as_deref(), Some("分页输入成功"));
        assert_eq!(updated.elements[4].reference, references[44]);
        session.close().await;
    }
}
