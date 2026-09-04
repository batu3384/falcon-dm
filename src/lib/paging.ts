// ponytail: cursor pagination over get_downloads (ORDER BY id DESC, id < before_id).

export function nextPageBeforeId(rows: { id: number }[], pageSize: number): number | undefined {
  if (pageSize <= 0 || rows.length < pageSize) return undefined;
  return rows[rows.length - 1]?.id;
}

export async function collectPaged<T extends { id: number }>(
  pageSize: number,
  fetchPage: (beforeId?: number) => Promise<T[]>,
): Promise<T[]> {
  const out: T[] = [];
  let beforeId: number | undefined;
  for (;;) {
    const rows = await fetchPage(beforeId);
    out.push(...rows);
    const next = nextPageBeforeId(rows, pageSize);
    if (next === undefined) break;
    beforeId = next;
  }
  return out;
}
