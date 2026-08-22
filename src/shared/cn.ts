import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** 合并 class：条件拼接后再按 Tailwind 规则消解冲突。 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
