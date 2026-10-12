// Finding task references ("ZUN-42") in text, for mentions in Markdown fields.
// Same form as the core (numbering::parse_ref): key of an upper-case letter and up
// to nine more upper-case letters or digits, a dash, a number without leading zero.

const REF_PATTERN = /(?<![\p{L}\p{N}_-])[A-Z][A-Z0-9]{0,9}-[1-9][0-9]{0,8}(?![\p{L}\p{N}_]|-[\p{L}\p{N}])/gu;

export interface RefMatch {
  ref: string;
  /** Offset in the text (UTF-16). */
  start: number;
  end: number;
}

/** All reference-shaped words in `text`, in order. Whether a task exists is for the caller to decide. */
export function findTaskRefs(text: string): RefMatch[] {
  const out: RefMatch[] = [];
  for (const m of text.matchAll(REF_PATTERN)) out.push({ ref: m[0], start: m.index, end: m.index + m[0].length });
  return out;
}
