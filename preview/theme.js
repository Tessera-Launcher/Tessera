// Theme selection is independent of sample application state and works on file://.
(() => {
  let mode = 'light';
  try {
    const saved = localStorage.getItem('tessera-color-scheme');
    if (saved === 'light' || saved === 'dark') mode = saved;
  } catch { /* Storage may be disabled for local files. */ }
  document.documentElement.dataset.theme = mode;
  document.addEventListener('DOMContentLoaded', () => {
    const select = document.querySelector('#theme-mode');
    select.value = mode;
    select.addEventListener('change', () => {
      mode = select.value === 'light' ? 'light' : 'dark';
      document.documentElement.dataset.theme = mode;
      try { localStorage.setItem('tessera-color-scheme', mode); } catch { /* Switching still works without persistence. */ }
    });
  });
})();
