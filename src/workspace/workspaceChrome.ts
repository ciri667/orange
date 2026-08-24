import type { ConfirmDialogConfig } from "../shared/ConfirmDialog";
import type { UserSettings, WorkspaceSnapshot } from "../shared/types";

/** 工作台动作 hook 共用的忙碌、确认和快照写入入口，状态仍由根组件持有。 */
export interface WorkspaceChrome {
  snapshot: WorkspaceSnapshot | null;
  userSettings: UserSettings | null;
  beginBusy: (label: string) => void;
  endBusy: () => void;
  setNotice: (notice: string) => void;
  commitSnapshot: (
    nextSnapshot: WorkspaceSnapshot,
    dirtyNotesToKeep?: Set<string>,
    dirtyDocumentsToKeep?: Set<string>,
  ) => void;
  requestConfirmation: (
    config: ConfirmDialogConfig,
    onConfirm: () => Promise<void> | void,
    onThirdAction?: () => Promise<void> | void,
  ) => void;
}
