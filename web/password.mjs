/** Accessible password entry; values are cleared before returning to the caller.
 * The UTF-8 bytes are sent as a text password (protocol 1.5), which the engine
 * encodes for the document's security handler. */
let activeCancel = null;
/** Dismiss a prompt when replacing or closing the document. */
export function cancelPassword() { activeCancel?.(); }

export function requestPassword(message = 'This PDF requires a password.') {
  const dialog = document.getElementById('password-dialog'), input = document.getElementById('password');
  const feedback = document.getElementById('password-feedback'), cancel = document.getElementById('password-cancel');
  const form = document.getElementById('password-form');
  const previousFocus = document.activeElement;
  feedback.textContent = message; input.value = '';
  return new Promise((resolve) => {
    const finish = (value) => {
      form.removeEventListener('submit', submit); cancel.removeEventListener('click', reject);
      dialog.removeEventListener('cancel', escape);
      activeCancel = null; input.value = ''; dialog.close(); previousFocus?.focus(); resolve(value);
    };
    const submit = (event) => { event.preventDefault(); finish(new TextEncoder().encode(input.value)); };
    const reject = () => finish(null);
    const escape = (event) => { event.preventDefault(); finish(null); };
    form.addEventListener('submit', submit); cancel.addEventListener('click', reject); dialog.addEventListener('cancel', escape);
    activeCancel = reject;
    dialog.showModal(); input.focus();
  });
}
