import { convertFileSrc } from "@tauri-apps/api/core";
import { MAX_PASTE_IMAGE_BATCH_BYTES, MAX_PASTE_IMAGE_BYTES, readImageFileAsBase64 } from "../workspace/editorPasteUtils";
import { createLocalId } from "../shared/id";
import type { ConversationImageAttachment, ConversationImageInput } from "../shared/types";

/** 单条消息最多携带的对话图片数，和 @ 文件上限对齐。 */
export const MAX_CONVERSATION_IMAGES = 8;

const SUPPORTED_IMAGE_MIME_TYPES = new Set(["image/png", "image/jpeg", "image/jpg", "image/webp", "image/gif"]);

/** 输入区未发送前的图片草稿；previewUrl 只存在于内存。 */
export interface ConversationImageDraft {
  localId: string;
  mimeType: string;
  byteSize: number;
  name?: string;
  previewUrl: string;
  bytesBase64: string;
}

function isTauriAssetRuntime() {
  if (typeof window === "undefined") {
    return false;
  }

  const tauriInternals = window.__TAURI_INTERNALS__;
  return typeof tauriInternals === "object" && tauriInternals !== null && "convertFileSrc" in tauriInternals;
}

function normalizeImageMimeType(value: string) {
  const normalized = value.split(";")[0]?.trim().toLowerCase() ?? "";
  if (normalized === "image/jpg") {
    return "image/jpeg";
  }
  return normalized;
}

function mimeTypeFromFileName(fileName: string) {
  const extension = fileName.split(".").pop()?.trim().toLowerCase();
  if (extension === "png") {
    return "image/png";
  }
  if (extension === "jpg" || extension === "jpeg") {
    return "image/jpeg";
  }
  if (extension === "webp") {
    return "image/webp";
  }
  if (extension === "gif") {
    return "image/gif";
  }
  return "";
}

/** 仅接受笔记粘贴同一组光栅格式，拒绝 SVG 等向量或伪装类型。 */
export function isSupportedConversationImageFile(file: File) {
  const mimeType = normalizeImageMimeType(file.type) || mimeTypeFromFileName(file.name);
  return SUPPORTED_IMAGE_MIME_TYPES.has(mimeType);
}

export function collectImageFilesFromList(files: File[]) {
  return files.filter(isSupportedConversationImageFile);
}

export function collectImageFilesFromDataTransfer(dataTransfer: DataTransfer | null) {
  return collectImageFilesFromList(Array.from(dataTransfer?.files ?? []));
}

export async function fileToConversationImageDraft(file: File): Promise<ConversationImageDraft> {
  const mimeType = normalizeImageMimeType(file.type) || mimeTypeFromFileName(file.name) || "image/png";
  return {
    localId: createLocalId("image"),
    mimeType,
    byteSize: file.size,
    name: file.name || undefined,
    previewUrl: URL.createObjectURL(file),
    bytesBase64: await readImageFileAsBase64(file),
  };
}

export function draftToConversationImageInput(draft: ConversationImageDraft): ConversationImageInput {
  return {
    mimeType: draft.mimeType,
    bytesBase64: draft.bytesBase64,
    originalFileName: draft.name,
  };
}

export function revokeConversationImagePreview(draft: ConversationImageDraft) {
  if (draft.previewUrl.startsWith("blob:")) {
    URL.revokeObjectURL(draft.previewUrl);
  }
}

export function conversationImageSrc(image: ConversationImageAttachment) {
  if (/^(data:|blob:|https?:)/i.test(image.absolutePath)) {
    return image.absolutePath;
  }

  return isTauriAssetRuntime() ? convertFileSrc(image.absolutePath) : "";
}

export function validateConversationImageFiles(
  incoming: File[],
  currentCount: number,
): { accepted: File[]; error?: string } {
  const supported = collectImageFilesFromList(incoming);
  if (!supported.length) {
    return { accepted: [], error: "仅支持 png、jpeg、webp 和 gif 图片。" };
  }

  if (currentCount + supported.length > MAX_CONVERSATION_IMAGES) {
    return {
      accepted: [],
      error: `单条消息最多上传 ${MAX_CONVERSATION_IMAGES} 张图片。`,
    };
  }

  if (supported.some((file) => file.size > MAX_PASTE_IMAGE_BYTES)) {
    return { accepted: [], error: "单张图片超过 20MB，已阻止添加。" };
  }

  const totalBytes = supported.reduce((sum, file) => sum + file.size, 0);
  if (totalBytes > MAX_PASTE_IMAGE_BATCH_BYTES) {
    return { accepted: [], error: "单次添加图片总大小超过 50MB，已阻止添加。" };
  }

  return { accepted: supported };
}
