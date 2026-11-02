// SVG geometry read from Manus, bundled locally (no Manus runtime requests).
// Stack: https://manus.im/ rendered “Create slides” control, 2026-11-02.
// Other controls: site/src/web/manus-icons.ts, captured 2026-10-17–20.
// Only currentColor, size and accessibility attributes are normalized.
const svg = (body: string) => `<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
export const WORKSET_STACK_ICON = svg('<path d="M6.99976 5.9974V4.26406C6.99976 3.38041 7.7161 2.66406 8.59976 2.66406H15.3998C16.2834 2.66406 16.9998 3.38041 16.9998 4.26406V5.9974"/><path d="M5.00024 10V8C5.00024 6.89543 5.89567 6 7.00024 6H17.0002C18.1048 6 19.0002 6.89543 19.0002 8V10"/><path d="M19 10H5C3.89543 10 3 10.8954 3 12V20C3 21.1046 3.89543 22 5 22H19C20.1046 22 21 21.1046 21 20V12C21 10.8954 20.1046 10 19 10Z"/>');
export const WORKSET_CLOSE_ICON = svg('<path d="M18 6 6 18"/><path d="m6 6 12 12"/>');
export const WORKSET_CHEVRON_ICON = svg('<path d="m9 18 6-6-6-6"/>');
export const WORKSET_REFRESH_ICON = svg('<path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/><path d="M8 16H3v5"/>');
const globe = svg('<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>');
const report = svg('<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z"/><path d="M14 2v5a1 1 0 0 0 1 1h5"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>');
const image = svg('<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>');
const film = svg('<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M7 3v18"/><path d="M3 7.5h4"/><path d="M3 12h18"/><path d="M3 16.5h4"/><path d="M17 3v18"/><path d="M17 7.5h4"/><path d="M17 16.5h4"/>');
const panel = svg('<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>');
const monitor = svg('<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>');
// This is the already captured Manus download icon, not a newly drawn variant.
const MANUS_DOWNLOAD_ICON = '<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18" fill="none" aria-hidden="true"><path fill="currentColor" d="M1.5 14.25V11.25C1.5 10.8358 1.83579 10.5 2.25 10.5C2.66421 10.5 3 10.8358 3 11.25V14.25C3 14.4489 3.07907 14.6396 3.21973 14.7803C3.36038 14.9209 3.55109 15 3.75 15H14.25C14.4489 15 14.6396 14.9209 14.7803 14.7803C14.9209 14.6396 15 14.4489 15 14.25V11.25C15 10.8358 15.3358 10.5 15.75 10.5C16.1642 10.5 16.5 10.8358 16.5 11.25V14.25C16.5 14.8467 16.2628 15.4189 15.8408 15.8408C15.4189 16.2628 14.8467 16.5 14.25 16.5H3.75C3.15326 16.5 2.58114 16.2628 2.15918 15.8408C1.73722 15.4189 1.5 14.8467 1.5 14.25ZM8.25 2.25C8.25 1.83579 8.58579 1.5 9 1.5C9.41421 1.5 9.75 1.83579 9.75 2.25V9.43945L12.2197 6.96973C12.5126 6.67683 12.9874 6.67683 13.2803 6.96973C13.5732 7.26262 13.5732 7.73738 13.2803 8.03027L9.53027 11.7803C9.23738 12.0732 8.76262 12.0732 8.46973 11.7803L4.71973 8.03027C4.42683 7.73738 4.42683 7.26262 4.71973 6.96973C5.01262 6.67683 5.48738 6.67683 5.78027 6.96973L8.25 9.43945V2.25Z"></path></svg>';
export function worksetIcon(id: string): string {
  if (id.endsWith('keyword-insights') || id.endsWith('-research')) return globe;
  if (id.endsWith('viral-analysis') || id.endsWith('image-text')) return image;
  if (id.endsWith('account-analysis')) return panel;
  if (id.endsWith('media-download')) return MANUS_DOWNLOAD_ICON;
  if (id.endsWith('video-transcript')) return film;
  if (id.endsWith('account-monitoring')) return monitor;
  if (id.endsWith('content-workflow')) return WORKSET_STACK_ICON;
  return report;
}
