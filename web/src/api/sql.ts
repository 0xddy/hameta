import { apiRequest } from "./client";

export interface SqlQueryResponse {
  executed_sql: string;
  columns: string[];
  rows: Array<Record<string, unknown>>;
  row_count: number;
  total: number | null;
  warning: string | null;
  took_ms: number;
}

export function executeReadonlySql(sql: string, maxRows = 200): Promise<SqlQueryResponse> {
  return apiRequest<SqlQueryResponse>("/api/v1/sql/query", {
    method: "POST",
    body: JSON.stringify({ sql, max_rows: maxRows }),
  });
}

function escapeCsvCell(val: unknown): string {
  if (val === null || val === undefined) return "";
  const str = typeof val === "object" ? JSON.stringify(val) : String(val);
  if (str.includes('"') || str.includes(",") || str.includes("\n") || str.includes("\r")) {
    return `"${str.replace(/"/g, '""')}"`;
  }
  return str;
}

export function exportRowsToCsv(
  columns: string[],
  rows: Array<Record<string, unknown>>,
  filename = "hameta-sql-result.csv",
) {
  const cols =
    columns.length > 0 ? columns : rows[0] ? Object.keys(rows[0]) : [];
  const header = cols.map(escapeCsvCell).join(",");
  const body = rows.map((row) => cols.map((c) => escapeCsvCell(row[c])).join(",")).join("\n");
  const csvContent = `\uFEFF${header}\n${body}`;
  const blob = new Blob([csvContent], { type: "text/csv;charset=utf-8;" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

export function exportRowsToJson(
  rows: Array<Record<string, unknown>>,
  filename = "hameta-sql-result.json",
) {
  const blob = new Blob([JSON.stringify(rows, null, 2)], {
    type: "application/json;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}
