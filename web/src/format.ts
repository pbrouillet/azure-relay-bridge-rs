export function dateTime(seconds?: number | null) {
  return seconds ? new Date(seconds * 1000).toLocaleString() : 'Never'
}
