// Types for scripts/i18n.mjs (used by the tests).
export interface Message { ctx: string; msgid: string; plural: string | null; where: string[] }
export interface Messages { ui: Map<string, Message>; buttons: Map<string, Message>; service: Map<string, Message> }
export interface PoEntry { ctx: string; msgid: string; plural: string | null; line: number; value: string | string[] }
export function parsePo(text: string): PoEntry[];
export function extractUi(files: string[], messages: Messages, errors: string[]): void;
export function extractRust(files: string[], messages: Messages): void;
export function uiMessages(): { messages: Messages; errors: string[] };
