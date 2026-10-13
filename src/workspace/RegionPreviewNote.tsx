import type { RegionIcon } from "./collapsibleRegions";

/**
 * 收起态悬浮浮层里的轻量说明卡。
 *
 * 用于「设置」「定时任务」这类不是一个可内嵌视图、但需要说明其内容的条目：
 * 浮层里给出标题和一句话描述，比塞一份残缺的界面更准确。
 */
export function RegionPreviewNote({
  title,
  description,
  Icon,
}: {
  title: string;
  description: string;
  Icon?: RegionIcon;
}) {
  return (
    <div className="region-preview-note">
      {Icon ? <Icon size={20} aria-hidden /> : null}
      <strong className="region-preview-note-title">{title}</strong>
      <p className="region-preview-note-description">{description}</p>
    </div>
  );
}
