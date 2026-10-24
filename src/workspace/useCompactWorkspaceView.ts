import { useCallback, useEffect, useState } from "react";

/** 小窗口一次只呈现笔记或对话；桌面布局偏好仍由原来的分栏 hook 管理。 */
export type CompactWorkspacePane = "editor" | "agent";

/** 与 app.css 同步的断点，不将小窗口的临时显隐写入桌面分栏偏好。 */
export function useCompactWorkspaceView() {
  const [compact, setCompact] = useState(() => window.matchMedia("(max-width: 1100px)").matches);
  const [singlePane, setSinglePane] = useState(() => window.matchMedia("(max-width: 760px)").matches);
  const [pane, setPane] = useState<CompactWorkspacePane>("editor");
  const [sidebarOpen, setSidebarOpen] = useState(false);

  useEffect(() => {
    const media = window.matchMedia("(max-width: 1100px)");
    const narrowMedia = window.matchMedia("(max-width: 760px)");
    const update = () => {
      setCompact(media.matches);
      setSinglePane(narrowMedia.matches);
      setSidebarOpen(false);
    };
    media.addEventListener("change", update);
    narrowMedia.addEventListener("change", update);
    return () => {
      media.removeEventListener("change", update);
      narrowMedia.removeEventListener("change", update);
    };
  }, []);

  useEffect(() => {
    if (!sidebarOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setSidebarOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [sidebarOpen]);

  const showPane = useCallback((nextPane: CompactWorkspacePane) => {
    setPane(nextPane);
    setSidebarOpen(false);
  }, []);

  return { compact, singlePane, pane, showPane, sidebarOpen, setSidebarOpen };
}
