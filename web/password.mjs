// @ts-check
/** Accessible password entry; values are cleared before returning to the caller. */
/** @type {(() => void) | null} */
let activeCancel = null;
/** Dismiss a prompt when replacing or closing the document. */
export function cancelPassword() {
  activeCancel?.();
}

/**
 * Resolve UTF-8 password bytes, or null when the user cancels.
 * @returns {Promise<Uint8Array | null>}
 */
export function requestPassword(message = 'This PDF requires a password.') {
  const dialog = /** @type {HTMLDialogElement} */ (document.getElementById('password-dialog')),
    input = /** @type {HTMLInputElement} */ (document.getElementById('password'));
  const feedback = /** @type {HTMLElement} */ (document.getElementById('password-feedback')),
    cancel = /** @type {HTMLButtonElement} */ (document.getElementById('password-cancel'));
  const form = /** @type {HTMLFormElement} */ (document.getElementById('password-form'));
  const previousFocus = /** @type {HTMLElement | null} */ (document.activeElement);
  feedback.textContent = message;
  input.value = '';
  return new Promise((resolve) => {
    /** @param {Uint8Array | null} value */
    const finish = (value) => {
      form.removeEventListener('submit', submit);
      cancel.removeEventListener('click', reject);
      dialog.removeEventListener('cancel', onEscape);
      activeCancel = null;
      input.value = '';
      dialog.close();
      previousFocus?.focus();
      resolve(value);
    };
    /** @param {SubmitEvent} event */
    const submit = (event) => {
      event.preventDefault();
      finish(new TextEncoder().encode(input.value));
    };
    const reject = () => finish(null);
    /** @param {Event} event */
    const onEscape = (event) => {
      event.preventDefault();
      finish(null);
    };
    form.addEventListener('submit', submit);
    cancel.addEventListener('click', reject);
    dialog.addEventListener('cancel', onEscape);
    activeCancel = reject;
    dialog.showModal();
    input.focus();
  });
}
