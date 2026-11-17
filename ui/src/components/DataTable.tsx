import { flexRender, getCoreRowModel, getFilteredRowModel, getPaginationRowModel, getSortedRowModel, useReactTable, type ColumnDef, type SortingState } from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ChevronsUpDown } from "lucide-react";
import { useState } from "react";
import { cn } from "@/lib/format";
import { Button, Empty, Input } from "./ui";

/** A sortable, filterable, paged table. Numbers align right (columns with meta.num). */
export default function DataTable<T>({
  data,
  columns,
  filterPlaceholder,
  pageSize = 50,
  initialSort,
  empty,
  onRowClick,
  rowClass,
}: {
  data: T[];
  columns: ColumnDef<T, unknown>[];
  filterPlaceholder?: string;
  pageSize?: number;
  initialSort?: SortingState;
  empty?: React.ReactNode;
  onRowClick?: (row: T) => void;
  rowClass?: (row: T) => string | undefined;
}) {
  const [sorting, setSorting] = useState<SortingState>(initialSort ?? []);
  const [filter, setFilter] = useState("");
  const table = useReactTable({
    data,
    columns,
    state: { sorting, globalFilter: filter },
    onSortingChange: setSorting,
    onGlobalFilterChange: setFilter,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    initialState: { pagination: { pageSize } },
  });
  const rows = table.getRowModel().rows;
  const total = table.getFilteredRowModel().rows.length;
  return (
    <div>
      {filterPlaceholder && (
        <div className="flex flex-wrap items-center gap-3 border-b border-line p-3">
          <Input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder={filterPlaceholder} aria-label={filterPlaceholder} className="max-w-sm" />
          <span className="text-xs text-muted tabular">
            {total === data.length ? `${data.length}` : `${total} of ${data.length}`}
          </span>
          {filter && (
            <Button size="sm" variant="ghost" onClick={() => setFilter("")}>
              Clear filter
            </Button>
          )}
        </div>
      )}
      {total === 0 ? (
        filter ? (
          <Empty title={`Nothing matches "${filter}"`}>
            <Button size="sm" onClick={() => setFilter("")}>
              Clear the filter
            </Button>
          </Empty>
        ) : (
          (empty ?? <Empty title="Nothing here yet" />)
        )
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              {table.getHeaderGroups().map((hg) => (
                <tr key={hg.id} className="border-b border-line">
                  {hg.headers.map((h) => {
                    const num = (h.column.columnDef.meta as { num?: boolean } | undefined)?.num;
                    const sort = h.column.getIsSorted();
                    return (
                      <th key={h.id} className={cn("px-3 py-2 text-xs font-medium text-muted whitespace-nowrap", num ? "text-right" : "text-left")} aria-sort={sort ? (sort === "asc" ? "ascending" : "descending") : undefined}>
                        {h.column.getCanSort() ? (
                          <button type="button" className={cn("inline-flex items-center gap-1 hover:text-text", num && "flex-row-reverse")} onClick={h.column.getToggleSortingHandler()}>
                            {flexRender(h.column.columnDef.header, h.getContext())}
                            {sort === "asc" ? <ArrowUp className="h-3 w-3" /> : sort === "desc" ? <ArrowDown className="h-3 w-3" /> : <ChevronsUpDown className="h-3 w-3 opacity-40" />}
                          </button>
                        ) : (
                          flexRender(h.column.columnDef.header, h.getContext())
                        )}
                      </th>
                    );
                  })}
                </tr>
              ))}
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr
                  key={r.id}
                  className={cn("border-b border-line last:border-0 hover:bg-panel-2", onRowClick && "cursor-pointer", rowClass?.(r.original))}
                  onClick={onRowClick ? () => onRowClick(r.original) : undefined}
                >
                  {r.getVisibleCells().map((c) => {
                    const num = (c.column.columnDef.meta as { num?: boolean } | undefined)?.num;
                    return (
                      <td key={c.id} className={cn("px-3 py-2 align-top", num && "text-right tabular whitespace-nowrap")}>
                        {flexRender(c.column.columnDef.cell, c.getContext())}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {table.getPageCount() > 1 && (
        <div className="flex items-center justify-end gap-2 border-t border-line p-2 text-xs text-muted">
          <span className="tabular">
            Page {table.getState().pagination.pageIndex + 1} of {table.getPageCount()}
          </span>
          <Button size="sm" variant="ghost" disabled={!table.getCanPreviousPage()} onClick={() => table.previousPage()}>
            Previous
          </Button>
          <Button size="sm" variant="ghost" disabled={!table.getCanNextPage()} onClick={() => table.nextPage()}>
            Next
          </Button>
        </div>
      )}
    </div>
  );
}
