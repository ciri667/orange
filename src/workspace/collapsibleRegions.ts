import { Clock, FolderTree, History, MessageSquare, PanelTop, Settings } from "lucide-react";
import type { ComponentType } from "react";

/** 工作台里可完全收起的区域：顶栏、左栏、右栏。中间的文档内容区始终存在，不参与收起。 */
export type RegionId = "top" | "sidebar" | "agent";

/** 区域收起后图标栏贴靠的边缘，决定图标栏排列方向和浮层展开方向。 */
export type RegionEdge = "left" | "right" | "top" | "bottom";

/** 收起态图标栏上的单个条目 id；一个区域可以有多个条目（如左栏的「会话」「文件」）。 */
export type RegionItemId = string;

/** lucide-react 图标组件的结构类型，只约束布局用到的属性，避免引入额外依赖。 */
export type RegionIcon = ComponentType<{ size?: number | string; className?: string; "aria-hidden"?: boolean }>;

/** 收起态图标栏上的一个图标；悬浮时浮层里渲染该条目的展开态内容。 */
export interface RegionItem {
  id: RegionItemId;
  /** 图标悬浮提示与无障碍名称，如「文件」。 */
  label: string;
  Icon: RegionIcon;
}

/** 收起态图标栏的统一厚度；与样式里的 --region-rail-size 保持一致。 */
export const REGION_RAIL_SIZE = 44;

/**
 * 四个可收起区域的静态声明。
 *
 * 这里只描述「区域是什么」，不描述「区域怎么渲染」：真实内容由 WorkspaceRegionContent
 * 按 RegionId 分发，这样展开态和收起态的悬浮浮层能共用同一份渲染入口。
 */
export const COLLAPSIBLE_REGIONS: Record<RegionId, {
  id: RegionId;
  edge: RegionEdge;
  /** 收起按钮与浮层的无障碍名称，如「知识库栏」。 */
  label: string;
  items: RegionItem[];
  /** 收起后残留的厚度（竖栏宽度或横向栏高度），单位 px。 */
  collapsedSize: number;
  defaultOpen: boolean;
}> = {
  top: {
    id: "top",
    edge: "top",
    label: "顶栏",
    items: [{ id: "main", label: "顶栏", Icon: PanelTop }],
    collapsedSize: REGION_RAIL_SIZE,
    defaultOpen: true,
  },
  sidebar: {
    id: "sidebar",
    edge: "left",
    label: "知识库栏",
    items: [
      { id: "sessions", label: "会话", Icon: MessageSquare },
      { id: "files", label: "文件", Icon: FolderTree },
      { id: "schedules", label: "定时任务", Icon: Clock },
      { id: "settings", label: "设置", Icon: Settings },
    ],
    collapsedSize: REGION_RAIL_SIZE,
    defaultOpen: true,
  },
  agent: {
    id: "agent",
    edge: "right",
    label: "Agent 协作区",
    items: [{ id: "main", label: "Agent 协作区", Icon: History }],
    collapsedSize: REGION_RAIL_SIZE,
    defaultOpen: true,
  },
};

/** 按固定顺序读取区域声明，保证图标栏和收起按钮的渲染顺序稳定。 */
export const REGION_ORDER: readonly RegionId[] = ["top", "sidebar", "agent"];

/** 读取某个区域的声明；RegionId 是闭合集合，这里不做空值兜底。 */
export function getRegion(regionId: RegionId) {
  return COLLAPSIBLE_REGIONS[regionId];
}
