export function minimalEdit(before: string, after: string) {
  let start = 0
  while (start < before.length && start < after.length && before[start] === after[start]) start++
  let end = before.length
  let nextEnd = after.length
  while (end > start && nextEnd > start && before[end - 1] === after[nextEnd - 1]) { end--; nextEnd-- }
  return { start, end, text: after.slice(start, nextEnd) }
}
