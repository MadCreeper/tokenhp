import type { TeamReport } from "./types";
export type TeamRange = "day" | "week" | "month";

/** Keep each range's data and in-flight request separate. Returning to a range
 * reuses its own request/cache, never another range's report. */
export class TeamReports {
  private cache = new Map<TeamRange, TeamReport>();
  private pending = new Map<TeamRange, Promise<TeamReport>>();
  private generation = 0;
  constructor(private load: (range: TeamRange) => Promise<TeamReport>) {}
  get(range: TeamRange): TeamReport | null { return this.cache.get(range) ?? null; }
  fetch(range: TeamRange): Promise<TeamReport> {
    const existing = this.pending.get(range);
    if (existing) return existing;
    const generation = this.generation;
    const request = this.load(range).then(report => {
      if (report.range !== range) throw new Error("Team response does not match the selected range.");
      if (generation === this.generation) this.cache.set(range, report);
      return report;
    }).finally(() => {
      if (this.pending.get(range) === request) this.pending.delete(range);
    });
    this.pending.set(range, request);
    return request;
  }
  clear(): void {
    this.generation++;
    this.cache.clear();
    this.pending.clear();
  }
}
