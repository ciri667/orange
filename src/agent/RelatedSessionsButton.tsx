import { MessagesSquare } from "lucide-react";
import { useState } from "react";
import { Button } from "../shared/Button";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import type { AgentSession } from "../shared/types";
import { sessionsTouchingFile, type SessionFileRef } from "./sessionListModel";

/** 文件头上的相关会话入口。没有引用、@ 或待确认变更时不显示。 */
export function RelatedSessionsButton({
  sessions,
  file,
  onSelectSession,
}: {
  sessions: AgentSession[];
  file: SessionFileRef;
  onSelectSession: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const related = sessionsTouchingFile(sessions, file);
  if (related.length === 0) {
    return null;
  }

  return (
    <Menu open={open} onClose={() => setOpen(false)}>
      <Button
        variant="text"
        title="打开引用或改过这个文件的会话"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <MessagesSquare size={16} />
        相关会话 {related.length}
      </Button>
      {open && (
        <MenuPanel placement="bottom-end" className="min-w-[220px]">
          {related.map((session) => (
            <MenuItem
              key={session.id}
              onClick={() => {
                setOpen(false);
                onSelectSession(session.id);
              }}
            >
              <span className="min-w-0 flex-1 truncate">{session.title}</span>
              {session.archivedAt ? <span className="shrink-0 text-[10px] text-ink-soft">归档</span> : null}
              <span className="shrink-0 text-[10px] text-ink-soft">{session.updatedAt}</span>
            </MenuItem>
          ))}
        </MenuPanel>
      )}
    </Menu>
  );
}
