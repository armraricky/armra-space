export function formatBytes(bytes: number): string {
  if (bytes === 0) return "—";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

export function formatDate(iso?: string): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function fileIcon(name: string, isDir: boolean): string {
  if (isDir) return "📁";
  const ext = name.split(".").pop()?.toLowerCase();
  const map: Record<string, string> = {
    png: "🖼️", jpg: "🖼️", jpeg: "🖼️", gif: "🖼️", webp: "🖼️", svg: "🖼️",
    mp4: "🎬", mov: "🎬", avi: "🎬", mkv: "🎬", webm: "🎬",
    mp3: "🎵", wav: "🎵", flac: "🎵", aac: "🎵",
    pdf: "📄", doc: "📝", docx: "📝", txt: "📝", md: "📝",
    zip: "📦", tar: "📦", gz: "📦", rar: "📦",
    json: "📋", ts: "💻", tsx: "💻", js: "💻", jsx: "💻",
    py: "💻", rs: "💻", go: "💻",
  };
  return map[ext ?? ""] ?? "📄";
}
