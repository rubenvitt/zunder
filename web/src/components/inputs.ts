/**
 * Für Eingaben, die keine Prosa sind (Befehle, Suche, Kurzsyntax, Adressen, Namen):
 * keine Rechtschreibprüfung, keine Autovervollständigung des Browsers und keine
 * Autokorrektur oder Großschreibung der Bildschirmtastatur (iOS/Android).
 */
export const PLAIN_INPUT = {
  autocomplete: "off",
  autocorrect: "off",
  autocapitalize: "off",
  spellcheck: false,
} as const;
