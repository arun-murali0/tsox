export type Severity = "Error" | "Warning";

/** One problem found in the checked source. Lines and columns are one-based; columns count UTF-16 units. */
export interface Diagnostic {
  severity: Severity;
  message: string;
  file_name: string;
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
}

export class TsoxChecker {
  constructor();
  /** Checks one TypeScript source text. Throws a string if it cannot be checked at all. */
  checkSource(source: string, fileName: string): Diagnostic[];
  free(): void;
}

/** Browsers and bundlers: call once, with the wasm URL or bytes, before `new TsoxChecker()`. */
export default function init(module?: unknown): Promise<unknown>;
