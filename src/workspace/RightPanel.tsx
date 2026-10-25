import { ChevronDown, Globe, MessageSquare, PanelRightClose, PanelRightOpen, Plus, TerminalSquare, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "../shared/Button";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { cn } from "../shared/cn";
import type { RightPanelTab } from "../shared/types";
import { rightTabId } from "./rightPanelModel";

export interface RightPanelTabItem { tab: RightPanelTab; title: string; status?: string }
const ICONS = { session: MessageSquare, terminal: TerminalSquare, browser: Globe };

/** 三类内容共用页签 chrome，内容实例由各领域维护。 */
export function RightPanel({ items, activeId, open, onExpand, onCollapse, onSelect, onClose, onCreate, onReorder, browserEnabled, children }: {
  items: RightPanelTabItem[];
  activeId: string | null;
  open: boolean;
  onExpand: () => void;
  onCollapse: () => void;
  onSelect: (tab: RightPanelTab) => void;
  onClose: (tab: RightPanelTab) => void;
  onCreate: (kind: RightPanelTab["kind"]) => void;
  onReorder: (source: string, target: string) => void;
  browserEnabled: boolean;
  children: ReactNode;
}) {
  const [newOpen, setNewOpen] = useState(false);
  const [listOpen, setListOpen] = useState(false);
  const strip = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const index = items.findIndex((item) => rightTabId(item.tab) === activeId);
    strip.current?.children[index]?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [activeId, items.length, open]);
  const create = (kind: RightPanelTab["kind"]) => { setNewOpen(false); onCreate(kind); };
  const newMenu = <Menu open={newOpen} onClose={() => setNewOpen(false)}>
    <Button variant="icon" title="新建内容" aria-label="新建内容" aria-haspopup="menu" aria-expanded={newOpen} onClick={() => setNewOpen(!newOpen)}><Plus size={16} /></Button>
    {newOpen && <MenuPanel>
      <MenuItem onClick={() => create("session")}><MessageSquare size={14} />新建会话</MenuItem>
      <MenuItem onClick={() => create("terminal")}><TerminalSquare size={14} />新建终端</MenuItem>
      <MenuItem onClick={() => create("browser")}><Globe size={14} />{browserEnabled ? "新建浏览器" : "浏览器：前往设置启用"}</MenuItem>
    </MenuPanel>}
  </Menu>;
  return <>
    <aside className="right-workbench" hidden={!open} aria-label="右侧内容区">
      <header className="right-panel-header">
        <div ref={strip} className="right-tab-strip" role="tablist" aria-label="右侧内容页签" onWheel={(event) => { if (strip.current && Math.abs(event.deltaY) > Math.abs(event.deltaX)) strip.current.scrollLeft += event.deltaY; }}>
          {items.map(({ tab, title, status }) => {
            const id = rightTabId(tab);
            const Icon = ICONS[tab.kind];
            return <div key={id} className={cn("right-tab", activeId === id && "is-active")} draggable onDragStart={(event) => { event.dataTransfer.setData("application/orange-tab", id); event.dataTransfer.effectAllowed = "move"; }} onDragOver={(event) => { if (event.dataTransfer.types.includes("application/orange-tab")) event.preventDefault(); }} onDrop={(event) => { event.preventDefault(); onReorder(event.dataTransfer.getData("application/orange-tab"), id); }}>
              <button className="right-tab-select" type="button" role="tab" aria-selected={activeId === id} title={`${title}${status ? ` · ${status}` : ""}`} onClick={() => onSelect(tab)}><Icon size={14} /><span>{title}</span>{status && <i className={cn("right-tab-status", status === "运行中" && "is-running")} aria-label={status} />}</button>
              <Button variant="icon" size="compact" className="right-tab-close" title={`关闭 ${title}`} aria-label={`关闭 ${title}`} onClick={() => onClose(tab)}><X size={12} /></Button>
            </div>;
          })}
        </div>
        <div className="right-panel-tools">
          <Menu open={listOpen} onClose={() => setListOpen(false)}>
            <Button variant="icon" title="全部页签" aria-label="全部页签" aria-haspopup="menu" aria-expanded={listOpen} disabled={!items.length} onClick={() => setListOpen(!listOpen)}><ChevronDown size={14} /></Button>
            {listOpen && <MenuPanel className="max-h-[min(420px,70vh)] overflow-y-auto">{items.map(({ tab, title, status }) => { const Icon = ICONS[tab.kind]; return <MenuItem key={rightTabId(tab)} onClick={() => { setListOpen(false); onSelect(tab); }}><Icon size={14} /><span className="max-w-[190px] truncate" title={title}>{title}</span>{status && <span className="text-ink-muted">{status}</span>}</MenuItem>; })}</MenuPanel>}
          </Menu>
          {open && newMenu}
          <Button variant="icon" title="收起右侧内容区" aria-label="收起右侧内容区" onClick={onCollapse}><PanelRightClose size={16} /></Button>
        </div>
      </header>
      <div className="right-panel-content">
        {!items.length && <div className="right-panel-empty"><span>打开一个内容开始工作</span><Button onClick={() => create("session")}><MessageSquare size={16} />新建会话</Button><Button onClick={() => create("terminal")}><TerminalSquare size={16} />新建终端</Button><Button onClick={() => create("browser")}><Globe size={16} />{browserEnabled ? "新建浏览器" : "启用浏览器"}</Button></div>}
        {children}
      </div>
    </aside>
    {!open && <aside className="region-rail region-rail-right right-content-rail" aria-label="收起的右侧内容区"><Button variant="icon" title="展开右侧内容区" aria-label="展开右侧内容区" onClick={onExpand}><PanelRightOpen size={18} /></Button>{newMenu}</aside>}
  </>;
}
