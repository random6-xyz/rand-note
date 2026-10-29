(() => {
  const API_BASE = (window.RAND_NOTE_CONFIG?.apiBaseUrl || "http://localhost:8787").replace(/\/$/, "");
  const HISTORY_START_DATE = "2026-10-14";
  const MAX_NOTE_LINES = 10;
  const MAX_NOTE_CHARS = 250;
  const MAX_RANGE_DAYS = 31;
  const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
  const ALLOWED_IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
  const state = { today: null, selectedRange: null, notes: [], isAdmin: false, image: null, previewUrl: null };

  const fromInput = document.querySelector("#history-from");
  const toInput = document.querySelector("#history-to");
  const viewDateButton = document.querySelector("#view-date");
  const viewTodayButton = document.querySelector("#view-today");
  const adminBadge = document.querySelector("#admin-badge");
  const resetTodayButton = document.querySelector("#reset-today");
  const boardHeading = document.querySelector("#board-heading");
  const boardStatus = document.querySelector("#board-status");
  const noteForm = document.querySelector("#note-form");
  const noteText = document.querySelector("#note-text");
  const submitButton = document.querySelector("#submit-note");
  const characterCount = document.querySelector("#character-count");
  const imageInput = document.querySelector("#note-image");
  const imagePreview = document.querySelector("#image-preview");
  const imagePreviewImg = document.querySelector("#image-preview-img");
  const removeImageButton = document.querySelector("#remove-image");
  const formStatus = document.querySelector("#form-status");
  const notesContainer = document.querySelector("#notes");

  function errorMessage(code) {
    const messages = {
      empty_text: "Write a note first.",
      too_many_lines: "Maximum 10 lines.",
      text_too_long: "Notes can be up to 250 characters.",
      invalid_json: "Could not read this note.",
      invalid_date: "Choose a valid date.",
      history_not_available: "History starts on 2026-10-14.",
      future_date: "Future dates are unavailable.",
      invalid_range: "Choose a start date on or before the end date.",
      range_too_long: `Choose a range of up to ${MAX_RANGE_DAYS} days.`,
      admin_required: "Admin cookie required.",
      origin_required: "Admin actions must come from this site.",
      rate_limited: "Too many posts. Try again in a moment.",
      image_too_large: "Images can be up to 5 MB.",
      unsupported_image: "Use a PNG, JPEG, GIF, or WebP image.",
      invalid_form: "Could not read this note.",
      internal_error: "Something went wrong. Try again."
    };
    return messages[code] || "Could not complete that request. Try again.";
  }

  async function api(path, options = {}) {
    const headers = { ...(options.headers || {}) };
    if (typeof options.body === "string") headers["content-type"] = "application/json";

    const response = await fetch(`${API_BASE}${path}`, {
      ...options,
      credentials: "include",
      headers
    });
    let body;
    try {
      body = await response.json();
    } catch {
      body = {};
    }
    if (!response.ok) {
      throw new Error(body.error || "request_failed");
    }
    return body;
  }

  function formatTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return "Unknown time";
    return new Intl.DateTimeFormat("en-GB", {
      timeZone: "Asia/Seoul",
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23"
    }).format(date);
  }

  function rangeLength(from, to) {
    return Math.round((Date.parse(to) - Date.parse(from)) / 86_400_000) + 1;
  }

  function isTodayView() {
    return state.selectedRange !== null
      && state.selectedRange.from === state.today
      && state.selectedRange.to === state.today;
  }

  function renderNotes(notes) {
    notesContainer.replaceChildren();
    if (notes.length === 0) {
      const empty = document.createElement("p");
      empty.className = "empty-state";
      empty.textContent = "No notes yet";
      notesContainer.append(empty);
      return;
    }

    for (const note of notes) {
      const card = document.createElement("article");
      card.className = "note-card";

      const contentViewport = document.createElement("div");
      contentViewport.className = "note-card-content";
      contentViewport.tabIndex = 0;

      if (note.image_type) {
        const imageUrl = `${API_BASE}/api/images/${encodeURIComponent(note.id)}`;
        const imageLink = document.createElement("a");
        imageLink.className = "note-image-link";
        imageLink.href = imageUrl;
        imageLink.target = "_blank";
        imageLink.rel = "noopener noreferrer";
        const image = document.createElement("img");
        image.className = "note-image";
        image.src = imageUrl;
        image.alt = note.text ? "Attached image" : "Image note";
        image.loading = "lazy";
        image.decoding = "async";
        imageLink.append(image);
        contentViewport.append(imageLink);
      }
      if (note.text) {
        const content = document.createElement("p");
        content.textContent = note.text;
        contentViewport.append(content);
      }

      const time = document.createElement("time");
      time.dateTime = note.created_at;
      const showDate = state.selectedRange && state.selectedRange.from !== state.selectedRange.to;
      time.textContent = showDate
        ? `${note.board_date} ${formatTime(note.created_at)}`
        : formatTime(note.created_at);

      card.append(contentViewport, time);
      if (state.isAdmin) {
        const deleteButton = document.createElement("button");
        deleteButton.className = "delete-note";
        deleteButton.type = "button";
        deleteButton.textContent = "×";
        deleteButton.setAttribute("aria-label", "Delete this note");
        deleteButton.addEventListener("click", () => deleteNote(note.id, deleteButton));
        card.append(deleteButton);
      }
      notesContainer.append(card);
    }
  }

  function setSelectedRange(from, to, notes) {
    state.selectedRange = { from, to };
    state.notes = notes;
    fromInput.value = from;
    toInput.value = to;
    toInput.min = from;
    boardHeading.textContent = from === to ? from : `${from} – ${to}`;
    noteForm.hidden = !isTodayView();
    if (isTodayView()) {
      boardStatus.textContent = state.today >= HISTORY_START_DATE ? "Today" : errorMessage("history_not_available");
    } else {
      const count = notes.length === 1 ? "1 note" : `${notes.length} notes`;
      boardStatus.textContent = `Archive · Read only · ${count}`;
    }
    renderNotes(notes);
  }

  function reloadSelectedRange() {
    if (isTodayView()) return loadToday();
    return loadRange(state.selectedRange.from, state.selectedRange.to);
  }

  async function refreshAdminSession() {
    try {
      const session = await api("/api/admin/session");
      state.isAdmin = session.authenticated === true;
    } catch {
      state.isAdmin = false;
    }
    adminBadge.hidden = !state.isAdmin;
    resetTodayButton.hidden = !state.isAdmin;
    if (state.selectedRange) renderNotes(state.notes);
  }

  async function deleteNote(id, button) {
    button.disabled = true;
    try {
      await api(`/api/admin/notes/${encodeURIComponent(id)}`, { method: "DELETE" });
      await reloadSelectedRange();
    } catch (error) {
      boardStatus.textContent = errorMessage(error.message);
      if (error.message === "admin_required") await refreshAdminSession();
    } finally {
      button.disabled = false;
    }
  }

  async function resetToday() {
    if (!window.confirm("Delete all notes from today's board?")) return;
    resetTodayButton.disabled = true;
    try {
      await api("/api/admin/notes/today", { method: "DELETE" });
      await loadToday();
    } catch (error) {
      boardStatus.textContent = errorMessage(error.message);
      if (error.message === "admin_required") await refreshAdminSession();
    } finally {
      resetTodayButton.disabled = false;
    }
  }

  async function loadToday() {
    boardStatus.textContent = "Loading…";
    notesContainer.setAttribute("aria-busy", "true");
    try {
      const data = await api("/api/notes");
      state.today = data.date;
      const historyUnavailable = data.date < HISTORY_START_DATE;
      fromInput.min = HISTORY_START_DATE;
      fromInput.max = data.date;
      toInput.max = data.date;
      fromInput.disabled = historyUnavailable;
      toInput.disabled = historyUnavailable;
      viewDateButton.disabled = historyUnavailable;
      setSelectedRange(data.date, data.date, data.notes);
      formStatus.textContent = "";
    } catch (error) {
      boardStatus.textContent = "Offline";
      notesContainer.replaceChildren();
      showEmptyError(error);
    } finally {
      notesContainer.setAttribute("aria-busy", "false");
    }
  }

  async function loadRange(from, to) {
    if (!from || !to) {
      boardStatus.textContent = "Choose a date range";
      return;
    }
    if (from < HISTORY_START_DATE || to < HISTORY_START_DATE) {
      boardStatus.textContent = errorMessage("history_not_available");
      return;
    }
    if (state.today && (from > state.today || to > state.today)) {
      boardStatus.textContent = errorMessage("future_date");
      return;
    }
    if (from > to) {
      boardStatus.textContent = errorMessage("invalid_range");
      return;
    }
    if (rangeLength(from, to) > MAX_RANGE_DAYS) {
      boardStatus.textContent = errorMessage("range_too_long");
      return;
    }
    boardStatus.textContent = "Loading…";
    notesContainer.setAttribute("aria-busy", "true");
    try {
      const params = new URLSearchParams({ from, to });
      const data = await api(`/api/notes?${params}`);
      setSelectedRange(data.from, data.to, data.notes);
    } catch (error) {
      boardStatus.textContent = "Unavailable";
      notesContainer.replaceChildren();
      showEmptyError(error);
    } finally {
      notesContainer.setAttribute("aria-busy", "false");
    }
  }

  function showEmptyError(error) {
    const message = document.createElement("p");
    message.className = "empty-state";
    message.textContent = errorMessage(error.message);
    notesContainer.append(message);
  }

  function countLines(text) {
    return text.length === 0 ? 0 : text.replace(/\r\n?/g, "\n").split("\n").length;
  }

  function updateCharacterCount() {
    const length = Array.from(noteText.value).length;
    const lines = countLines(noteText.value);
    characterCount.textContent = `${lines}/10 lines · ${length}/250 chars`;
    characterCount.classList.toggle("over-limit", lines > MAX_NOTE_LINES || length > MAX_NOTE_CHARS);
    noteText.style.height = "auto";
    noteText.style.height = `${noteText.scrollHeight}px`;
  }

  function enforceLineLimit(event) {
    if (event.isComposing) {
      updateCharacterCount();
      return;
    }

    if (countLines(noteText.value) > MAX_NOTE_LINES) {
      const caret = noteText.selectionStart;
      noteText.value = noteText.value.replace(/\r\n?/g, "\n").split("\n").slice(0, MAX_NOTE_LINES).join("\n");
      const nextCaret = Math.min(caret, noteText.value.length);
      noteText.setSelectionRange(nextCaret, nextCaret);
      formStatus.textContent = errorMessage("too_many_lines");
    } else if (formStatus.textContent === errorMessage("too_many_lines")) {
      formStatus.textContent = "";
    }

    updateCharacterCount();
  }

  function clearImage() {
    if (state.previewUrl) URL.revokeObjectURL(state.previewUrl);
    state.image = null;
    state.previewUrl = null;
    imageInput.value = "";
    imagePreviewImg.removeAttribute("src");
    imagePreview.hidden = true;
  }

  function selectImage(file) {
    if (!ALLOWED_IMAGE_TYPES.includes(file.type)) {
      clearImage();
      formStatus.textContent = errorMessage("unsupported_image");
      return;
    }
    if (file.size > MAX_IMAGE_BYTES) {
      clearImage();
      formStatus.textContent = errorMessage("image_too_large");
      return;
    }
    if (state.previewUrl) URL.revokeObjectURL(state.previewUrl);
    state.image = file;
    state.previewUrl = URL.createObjectURL(file);
    imagePreviewImg.src = state.previewUrl;
    imagePreview.hidden = false;
    formStatus.textContent = "";
  }

  viewDateButton.addEventListener("click", () => loadRange(fromInput.value, toInput.value));
  viewTodayButton.addEventListener("click", loadToday);
  resetTodayButton.addEventListener("click", resetToday);
  fromInput.addEventListener("change", () => {
    toInput.min = fromInput.value || HISTORY_START_DATE;
    if (fromInput.value && (!toInput.value || toInput.value < fromInput.value)) {
      toInput.value = fromInput.value;
    }
  });
  for (const input of [fromInput, toInput]) {
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        loadRange(fromInput.value, toInput.value);
      }
    });
  }

  noteText.addEventListener("input", enforceLineLimit);
  imageInput.addEventListener("change", () => {
    const [file] = imageInput.files;
    if (file) selectImage(file);
  });
  removeImageButton.addEventListener("click", () => {
    clearImage();
    noteText.focus();
  });
  noteText.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      if (!submitButton.disabled) noteForm.requestSubmit();
    }
  });

  noteForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const text = noteText.value.trim();
    if (!text && !state.image) {
      formStatus.textContent = errorMessage("empty_text");
      noteText.focus();
      return;
    }
    if (countLines(text) > MAX_NOTE_LINES) {
      formStatus.textContent = errorMessage("too_many_lines");
      noteText.focus();
      return;
    }
    if (Array.from(text).length > MAX_NOTE_CHARS) {
      formStatus.textContent = errorMessage("text_too_long");
      noteText.focus();
      return;
    }

    submitButton.disabled = true;
    formStatus.textContent = "Posting…";
    try {
      let body;
      if (state.image) {
        body = new FormData();
        body.append("text", text);
        body.append("image", state.image);
      } else {
        body = JSON.stringify({ text });
      }
      await api("/api/notes", { method: "POST", body });
      noteText.value = "";
      clearImage();
      updateCharacterCount();
      await loadToday();
      formStatus.textContent = "Posted.";
    } catch (error) {
      formStatus.textContent = errorMessage(error.message);
    } finally {
      submitButton.disabled = false;
    }
  });

  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && isTodayView()) loadToday();
  });

  updateCharacterCount();
  refreshAdminSession().then(loadToday);
})();
