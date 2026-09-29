import { t } from "./i18n";
import xhsIcon from "../../../site/public/platforms/xiaohongshu.png";
import videoIcon from "../../../site/public/platforms/tiktok.png";
import instagramIcon from "../../../site/public/platforms/instagram.png";
import linkedinIcon from "../../../site/public/platforms/linkedin.svg";
import xIcon from "../assets/platforms/x.svg?raw";

const SOURCE_STORAGE_KEY = "socai-research-sources";
const icons: Record<string, string> = {
  xhs: xhsIcon, dy: videoIcon, tiktok: videoIcon,
  instagram: instagramIcon, linkedin: linkedinIcon, x: xIcon,
};

export function platformIcon(site: string): string {
  if (site === "auto") {
    return `<span class="platform-icon" aria-hidden="true"><svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.5"><path d="m10 1.5 2.1 6.4L18.5 10l-6.4 2.1L10 18.5l-2.1-6.4L1.5 10l6.4-2.1L10 1.5Z"/></svg></span>`;
  }
  const icon = icons[site];
  const content = site === "x" ? xIcon : icon ? `<img src="${icon}" alt="" />` : "";
  return `<span class="platform-icon" aria-hidden="true">${content}</span>`;
}

export function savedSources(): string[] | undefined {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SOURCE_STORAGE_KEY) ?? "null");
    if (!Array.isArray(value)) return undefined;
    const sites = RESEARCH_PLATFORMS.filter((site) => value.includes(site));
    if (sites.includes("auto")) return ["auto"];
    return sites.length ? [...sites] : undefined;
  } catch { return undefined; }
}

export function saveSources(sites: string[]): void {
  try { localStorage.setItem(SOURCE_STORAGE_KEY, JSON.stringify(sites)); } catch { /* Session state still works. */ }
}

export const RESEARCH_PLATFORMS = ["auto", "xhs", "dy", "tiktok", "instagram", "linkedin", "x"] as const;

export function platformLabel(site: string): string {
  switch (site) {
    case "auto": return t("platform.auto");
    case "xhs": return t("platform.xhs");
    case "dy": return t("platform.dy");
    case "tiktok": return "TikTok";
    case "instagram": return "Instagram";
    case "linkedin": return "LinkedIn";
    case "x": return "X";
    default: return site;
  }
}
