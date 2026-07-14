import { ChevronLeft, ChevronRight, FileImage, FileText, FileType2, NotebookPen, X } from "lucide-react";
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { logDebug, logInfo } from "../shared/logger";
import type { DocumentFileType, EditorFileTab } from "../shared/types";

/** 标签栏渲染所需的前端展示信息；文件实体仍由工作区状态统一维护。 */
export type EditorTabBarItem = EditorFileTab & {
  title: string;
  fileType?: DocumentFileType;
  isDirty?: boolean;
};

/** 横向滚动来源，用于让上层做轻量可观测统计。 */
export type EditorTabScrollSource = "button" | "keyboard";

/** 共享编辑器标签栏的受控属性，打开、保存及关闭决策均由工作区处理。 */
export interface EditorTabBarProps {
  tabs: EditorTabBarItem[];
  activeTab: EditorFileTab | null;
  onSelect: (tab: EditorFileTab) => void;
  onClose: (tab: EditorFileTab) => void;
  onScroll?: (direction: "left" | "right", source: EditorTabScrollSource) => void;
}

/** 将标签的种类和文件 ID 组合成稳定键，避免不同类型文件碰撞。 */
function getTabKey(tab: EditorFileTab) {
  return `${tab.kind}:${tab.id}`;
}

/** 根据文件类型选取与文件树一致的轻量图标。 */
function TabFileIcon({ tab }: { tab: EditorTabBarItem }) {
  if (tab.kind === "note") {
    return <NotebookPen size={15} aria-hidden="true" />;
  }

  if (tab.fileType === "image") {
    return <FileImage size={15} aria-hidden="true" />;
  }

  if (tab.fileType === "docx") {
    return <FileType2 size={15} aria-hidden="true" />;
  }

  return <FileText size={15} aria-hidden="true" />;
}

/** IDE 风格的多文件标签栏，负责可访问键盘导航和溢出滚动，不持有文件业务状态。 */
export function EditorTabBar({ tabs, activeTab, onSelect, onClose, onScroll }: EditorTabBarProps) {
  const tabListRef = useRef<HTMLDivElement>(null);
  const tabRefs = useRef(new Map<string, HTMLButtonElement>());
  const focusAfterSelectRef = useRef<string | null>(null);
  const [canScrollLeft, setCanScrollLeft] = useState(false);
  const [canScrollRight, setCanScrollRight] = useState(false);
  const tabListId = useId();
  const activeKey = activeTab ? getTabKey(activeTab) : null;

  /** 根据实际滚动范围同步箭头状态，避免在边界仍提供无效操作。 */
  const updateScrollState = useCallback(() => {
    const element = tabListRef.current;

    if (!element) {
      return;
    }

    // 浏览器浮点布局可能留下极小误差，因此以 1px 容差判断滚动边界。
    const maxScrollLeft = Math.max(0, element.scrollWidth - element.clientWidth);
    setCanScrollLeft(element.scrollLeft > 1);
    setCanScrollRight(element.scrollLeft < maxScrollLeft - 1);
  }, []);

  /** 激活标签应始终处于可视范围，切换文件时不要求用户手动寻找对应标签。 */
  useLayoutEffect(() => {
    if (!activeKey) {
      return;
    }

    tabRefs.current.get(activeKey)?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "nearest" });
    updateScrollState();
  }, [activeKey, updateScrollState]);

  useEffect(() => {
    const element = tabListRef.current;

    if (!element) {
      return;
    }

    updateScrollState();
    const observer = new ResizeObserver(updateScrollState);
    observer.observe(element);

    return () => observer.disconnect();
  }, [tabs.length, updateScrollState]);

  /** 受控激活状态更新完成后，把键盘焦点还给目标标签。 */
  useEffect(() => {
    const focusKey = focusAfterSelectRef.current;

    if (!focusKey || focusKey !== activeKey) {
      return;
    }

    tabRefs.current.get(focusKey)?.focus();
    focusAfterSelectRef.current = null;
  }, [activeKey]);

  /** 选择标签并记录不含文件名、路径和正文的关键交互日志。 */
  const selectTab = useCallback(
    (tab: EditorTabBarItem, shouldFocus = false) => {
      const tabKey = getTabKey(tab);
      if (shouldFocus) {
        focusAfterSelectRef.current = tabKey;
      }

      logInfo("切换编辑器文件标签。", {
        category: "frontend",
        event: "editor_tab_select",
        metadata: { kind: tab.kind, tabCount: tabs.length, isDirty: Boolean(tab.isDirty) },
      });
      onSelect(tab);
    },
    [onSelect, tabs.length],
  );

  /** 关闭由上层执行；脏草稿的保存、放弃或取消确认不能在展示组件中绕过。 */
  const closeTab = useCallback(
    (tab: EditorTabBarItem) => {
      logInfo("请求关闭编辑器文件标签。", {
        category: "frontend",
        event: "editor_tab_close",
        metadata: { kind: tab.kind, tabCount: tabs.length, isDirty: Boolean(tab.isDirty) },
      });
      onClose(tab);
    },
    [onClose, tabs.length],
  );

  /** 按一个可视区宽度移动，保留原生滚动和触控板横滑行为。 */
  const scrollTabs = useCallback(
    (direction: "left" | "right", source: EditorTabScrollSource) => {
      const element = tabListRef.current;
      if (!element) {
        return;
      }

      const distance = Math.max(160, Math.floor(element.clientWidth * 0.8));
      element.scrollBy({ left: direction === "left" ? -distance : distance, behavior: "smooth" });
      logDebug("滚动编辑器文件标签。", {
        category: "frontend",
        event: "editor_tab_scroll",
        metadata: { direction, source, tabCount: tabs.length },
      });
      onScroll?.(direction, source);
    },
    [onScroll, tabs.length],
  );

  /** 处理符合 ARIA tabs 模式的方向、首尾和关闭快捷键。 */
  const handleTabKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    const lowerKey = event.key.toLowerCase();
    const shouldClose = event.key === "Delete" || event.key === "Backspace" || ((event.metaKey || event.ctrlKey) && lowerKey === "w");

    if (shouldClose) {
      event.preventDefault();
      const tab = tabs[index];
      if (tab) {
        closeTab(tab);
      }
      return;
    }

    let nextIndex: number | null = null;
    if (event.key === "ArrowLeft") {
      nextIndex = Math.max(0, index - 1);
      scrollTabs("left", "keyboard");
    } else if (event.key === "ArrowRight") {
      nextIndex = Math.min(tabs.length - 1, index + 1);
      scrollTabs("right", "keyboard");
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = tabs.length - 1;
    }

    if (nextIndex === null) {
      return;
    }

    event.preventDefault();
    const nextTab = tabs[nextIndex];
    if (nextTab) {
      selectTab(nextTab, true);
    }
  };

  if (!tabs.length) {
    return null;
  }

  return (
    <div className="editor-tab-bar" aria-label="已打开文件">
      <button
        className="editor-tab-scroll-button"
        type="button"
        aria-label="向左滚动文件标签"
        disabled={!canScrollLeft}
        onClick={() => scrollTabs("left", "button")}
      >
        <ChevronLeft size={16} aria-hidden="true" />
      </button>
      <div
        className="editor-tab-list"
        id={tabListId}
        ref={tabListRef}
        role="tablist"
        aria-label="已打开文件标签"
        onScroll={updateScrollState}
      >
        {tabs.map((tab, index) => {
          const tabKey = getTabKey(tab);
          const isActive = tabKey === activeKey;
          const tabId = `${tabListId}-${index}`;

          return (
            <div className={`editor-tab ${isActive ? "active" : ""}`} key={tabKey}>
              <button
                className="editor-tab-select"
                id={tabId}
                ref={(element) => {
                  if (element) {
                    tabRefs.current.set(tabKey, element);
                  } else {
                    tabRefs.current.delete(tabKey);
                  }
                }}
                type="button"
                role="tab"
                aria-selected={isActive}
                aria-controls="editor-file-panel"
                tabIndex={isActive ? 0 : -1}
                title={tab.title}
                onClick={() => selectTab(tab)}
                onKeyDown={(event) => handleTabKeyDown(event, index)}
              >
                <TabFileIcon tab={tab} />
                <span className="editor-tab-title">{tab.title}</span>
                {tab.isDirty && <span className="editor-tab-dirty" aria-label="包含未保存的更改" />}
              </button>
              <button
                className="editor-tab-close"
                type="button"
                aria-label={`关闭 ${tab.title}`}
                title={`关闭 ${tab.title}`}
                onClick={() => closeTab(tab)}
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>
          );
        })}
      </div>
      <button
        className="editor-tab-scroll-button"
        type="button"
        aria-label="向右滚动文件标签"
        disabled={!canScrollRight}
        onClick={() => scrollTabs("right", "button")}
      >
        <ChevronRight size={16} aria-hidden="true" />
      </button>
    </div>
  );
}
