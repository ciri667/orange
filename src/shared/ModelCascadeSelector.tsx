import { Check, ChevronDown, ChevronRight } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { logDebug } from "./logger";
import {
  decodeModelSelection,
  encodeModelSelection,
  FOLLOW_DEFAULT_MODEL_SELECTION,
  getProviderModelLabel,
  getProviderModelSelectionLabel,
  getSelectableModels,
} from "./modelSelection";
import { OverflowTooltipText } from "./OverflowTooltipText";
import type { LlmProviderConfig } from "./types";
import { useDismissable } from "./useDismissable";

/** 级联模型选择器的触发入口样式；inline 用于 Agent 输入区，block 用于设置/上下文表单。 */
type ModelCascadeSelectorVariant = "inline" | "block";

/** 级联模型选择器 props，value 沿用 provider/model 编码，空字符串表示跟随默认。 */
interface ModelCascadeSelectorProps {
  value: string;
  providers: LlmProviderConfig[];
  defaultLabel: string;
  ariaLabel: string;
  onChange: (selection: string) => void;
  className?: string;
  triggerPrefix?: string;
  variant?: ModelCascadeSelectorVariant;
  disabled?: boolean;
  logArea: string;
}

/** Provider 列表项预计算结果，避免渲染时重复扫描模型列表。 */
interface ProviderModelGroup {
  provider: LlmProviderConfig;
  models: ReturnType<typeof getSelectableModels>;
}

/** 浮层和视口边缘的最小距离，避免菜单贴边或被裁剪。 */
const MENU_VIEWPORT_MARGIN = 12;
/** 浮层与触发按钮之间的距离。 */
const MENU_TRIGGER_GAP = 8;
/** 级联菜单设计宽度；窄屏会按视口自动收缩。 */
const MENU_DESIRED_WIDTH = 540;
/** 首次定位时使用的菜单预估高度，二次 layout 会用真实高度校正。 */
const MENU_ESTIMATED_HEIGHT = 230;

/** 根据当前选择和 provider 列表找出初始展开 provider；无选择时默认展开第一个有模型的 provider。 */
function resolveInitialActiveProviderId(value: string, groups: ProviderModelGroup[]) {
  const decodedSelection = decodeModelSelection(value);

  if (decodedSelection.providerId && groups.some((group) => group.provider.id === decodedSelection.providerId)) {
    return decodedSelection.providerId;
  }

  return groups.find((group) => group.models.length > 0)?.provider.id ?? groups[0]?.provider.id ?? "";
}

/** 生成触发器展示文案；默认项展示外部传入文案，模型项展示 Provider / Model。 */
function resolveTriggerLabel(
  value: string,
  groups: ProviderModelGroup[],
  defaultLabel: string,
  triggerPrefix: string,
) {
  const decodedSelection = decodeModelSelection(value);
  const provider = groups.find((group) => group.provider.id === decodedSelection.providerId)?.provider;

  if (!decodedSelection.providerId || !decodedSelection.modelId || !provider) {
    return defaultLabel;
  }

  return `${triggerPrefix}${getProviderModelSelectionLabel(provider, decodedSelection.modelId)}`;
}

/** 两栏级联模型选择器：左侧 Provider，悬停/聚焦后右侧展示该 Provider 的模型。 */
export function ModelCascadeSelector({
  value,
  providers,
  defaultLabel,
  ariaLabel,
  onChange,
  className = "",
  triggerPrefix = "",
  variant = "inline",
  disabled = false,
  logArea,
}: ModelCascadeSelectorProps) {
  /** 只保留至少有一个可选模型的 provider，避免右侧出现空模型面板。 */
  const groups = useMemo(
    () =>
      providers
        .map<ProviderModelGroup>((provider) => ({ provider, models: getSelectableModels(provider) }))
        .filter((group) => group.models.length > 0),
    [providers],
  );
  /** 控制菜单展开；点击外部或 Esc 会关闭。 */
  const [isOpen, setIsOpen] = useState(false);
  /** 当前左侧悬停/聚焦的 provider ID，右侧模型列表由它驱动。 */
  const [activeProviderId, setActiveProviderId] = useState(() => resolveInitialActiveProviderId(value, groups));
  /** 触发按钮用于计算 body 级浮层的 fixed 坐标。 */
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  /** 菜单通过 portal 渲染到 body，需要额外纳入 dismissable 的内部区域。 */
  const menuRef = useRef<HTMLDivElement | null>(null);
  /** 菜单 fixed 定位样式；打开后会按按钮和视口实时计算。 */
  const [menuStyle, setMenuStyle] = useState<CSSProperties | null>(null);
  const wrapperRef = useDismissable<HTMLDivElement>(isOpen, () => setIsOpen(false), { insideRefs: [menuRef] });
  const decodedSelection = decodeModelSelection(value);
  const activeGroup = groups.find((group) => group.provider.id === activeProviderId) ?? groups[0];
  const triggerLabel = resolveTriggerLabel(value, groups, defaultLabel, triggerPrefix);

  /** 将菜单定位到触发器附近，并按视口边界回退，避免侧栏 overflow 裁剪菜单。 */
  const updateMenuPosition = useCallback(() => {
    const trigger = triggerRef.current;

    if (!trigger) {
      return;
    }

    const triggerRect = trigger.getBoundingClientRect();
    const menuRect = menuRef.current?.getBoundingClientRect();
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const width = Math.min(MENU_DESIRED_WIDTH, Math.max(260, viewportWidth - MENU_VIEWPORT_MARGIN * 2));
    const height = menuRect?.height || MENU_ESTIMATED_HEIGHT;
    const preferredLeft = variant === "inline" ? triggerRect.right - width : triggerRect.left;
    const left = Math.min(
      Math.max(MENU_VIEWPORT_MARGIN, preferredLeft),
      Math.max(MENU_VIEWPORT_MARGIN, viewportWidth - width - MENU_VIEWPORT_MARGIN),
    );
    const topBelow = triggerRect.bottom + MENU_TRIGGER_GAP;
    const topAbove = triggerRect.top - height - MENU_TRIGGER_GAP;
    const preferredTop =
      variant === "inline" || topBelow + height > viewportHeight - MENU_VIEWPORT_MARGIN ? topAbove : topBelow;
    const fallbackTop = topBelow + height <= viewportHeight - MENU_VIEWPORT_MARGIN ? topBelow : topAbove;
    const top = Math.min(
      Math.max(MENU_VIEWPORT_MARGIN, preferredTop >= MENU_VIEWPORT_MARGIN ? preferredTop : fallbackTop),
      Math.max(MENU_VIEWPORT_MARGIN, viewportHeight - height - MENU_VIEWPORT_MARGIN),
    );

    setMenuStyle({ left, top, width });
  }, [variant]);

  useLayoutEffect(() => {
    if (isOpen) {
      updateMenuPosition();
    } else {
      setMenuStyle(null);
    }
  }, [activeProviderId, isOpen, updateMenuPosition]);

  useEffect(() => {
    if (!isOpen) {
      return undefined;
    }

    window.addEventListener("resize", updateMenuPosition);
    window.addEventListener("scroll", updateMenuPosition, true);

    return () => {
      window.removeEventListener("resize", updateMenuPosition);
      window.removeEventListener("scroll", updateMenuPosition, true);
    };
  }, [isOpen, updateMenuPosition]);

  /** 打开菜单时同步当前选中 provider，保证右侧模型列直接定位到当前上下文。 */
  function handleToggleOpen() {
    const nextOpen = !isOpen;

    if (nextOpen) {
      setActiveProviderId(resolveInitialActiveProviderId(value, groups));
    }

    setIsOpen(nextOpen);
    logDebug("切换模型级联选择器。", {
      category: "frontend",
      event: "model_cascade_selector_toggle",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        logArea,
        providerCount: groups.length,
        hasExplicitSelection: Boolean(decodedSelection.providerId && decodedSelection.modelId),
      },
    });
  }

  /** 写入选择结果并关闭菜单；只记录 provider/model ID，不包含密钥或请求正文。 */
  function handleSelect(selection: string, providerId?: string, modelId?: string) {
    onChange(selection);
    setIsOpen(false);
    logDebug("选择模型级联项。", {
      category: "frontend",
      event: "model_cascade_selector_select",
      status: "completed",
      metadata: {
        logArea,
        providerId: providerId ?? "",
        modelId: modelId ?? "",
        isDefaultSelection: selection === FOLLOW_DEFAULT_MODEL_SELECTION,
      },
    });
  }

  return (
    <div ref={wrapperRef} className={`model-cascade ${variant} ${className}`} data-open={isOpen || undefined}>
      <button
        ref={triggerRef}
        className="model-cascade-trigger"
        type="button"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={handleToggleOpen}
      >
        <OverflowTooltipText as="span" text={triggerLabel} logArea={`${logArea}_trigger`} />
        <ChevronDown size={13} />
      </button>
      {isOpen &&
        createPortal(
          <div
            ref={menuRef}
            className="model-cascade-menu"
            role="menu"
            aria-label={ariaLabel}
            style={menuStyle ?? { visibility: "hidden" }}
          >
            <div className="model-cascade-provider-list" role="group" aria-label="Provider">
              <button
                className={`model-cascade-provider-row default ${value === FOLLOW_DEFAULT_MODEL_SELECTION ? "selected" : ""}`}
                type="button"
                role="menuitemradio"
                aria-checked={value === FOLLOW_DEFAULT_MODEL_SELECTION}
                onClick={() => handleSelect(FOLLOW_DEFAULT_MODEL_SELECTION)}
              >
                <span>
                  <OverflowTooltipText text={defaultLabel} logArea={`${logArea}_default`} />
                </span>
                {value === FOLLOW_DEFAULT_MODEL_SELECTION && <Check size={15} />}
              </button>
              {groups.map((group) => {
                const isActive = activeGroup?.provider.id === group.provider.id;
                const isSelectedProvider = decodedSelection.providerId === group.provider.id;

                return (
                  <button
                    className={`model-cascade-provider-row ${isActive ? "active" : ""} ${isSelectedProvider ? "selected" : ""}`}
                    key={group.provider.id}
                    type="button"
                    role="menuitem"
                    onFocus={() => setActiveProviderId(group.provider.id)}
                    onMouseEnter={() => setActiveProviderId(group.provider.id)}
                  >
                    <span>
                      <OverflowTooltipText as="strong" text={group.provider.name} logArea={`${logArea}_provider_name`} />
                      <small>{group.models.length} 个模型</small>
                    </span>
                    <ChevronRight size={15} />
                  </button>
                );
              })}
            </div>
            <div className="model-cascade-model-list" role="group" aria-label="模型">
              <div className="model-cascade-list-heading">模型</div>
              {activeGroup ? (
                activeGroup.models.map((model) => {
                  const selection = encodeModelSelection(activeGroup.provider.id, model.id);
                  const isSelected = value === selection;
                  const modelLabel = getProviderModelLabel(activeGroup.provider, model.id);
                  const shouldShowModelId = modelLabel !== model.id;

                  return (
                    <button
                      className={`model-cascade-model-row ${isSelected ? "selected" : ""}`}
                      key={`${activeGroup.provider.id}:${model.id}`}
                      type="button"
                      role="menuitemradio"
                      aria-checked={isSelected}
                      onClick={() => handleSelect(selection, activeGroup.provider.id, model.id)}
                    >
                      <span>
                        <OverflowTooltipText as="strong" text={modelLabel} logArea={`${logArea}_model_name`} />
                        {shouldShowModelId && (
                          <OverflowTooltipText as="small" text={model.id} logArea={`${logArea}_model_id`} />
                        )}
                      </span>
                      {isSelected && <Check size={15} />}
                    </button>
                  );
                })
              ) : (
                <div className="model-cascade-empty">暂无可选模型</div>
              )}
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}
