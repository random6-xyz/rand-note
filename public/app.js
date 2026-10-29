(() => {
  const API_BASE = (window.RAND_NOTE_CONFIG?.apiBaseUrl || "http://localhost:8787").replace(/\/$/, "");
  const HISTORY_START_DATE = "2026-10-14";
  const MAX_NOTE_LINES = 10;
  const MAX_NOTE_CHARS = 250;
  const GRID_ROW_PX = 4;
  const state = { today: null, selectedDate: null, notes: [], isAdmin: false };

  const dateInput = document.querySelector("#history-date");
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
  const formStatus = document.querySelector("#form-status");
  const notesContainer = document.querySelector("#notes");

  // Each card spans enough grid rows for its height plus the column gap, so short
  // cards are not stretched to the tallest card in a row and no blank holes remain.
  const cardResizeObserver = new ResizeObserver((entries) => {
    const gap = parseFloat(getComputedStyle(notesContainer).columnGap) || 0;
    for (const { target } of entries) {
      target.style.gridRowEnd = `span ${Math.ceil((target.offsetHeight + gap) / GRID_ROW_PX)}`;
    }
  });

  function errorMessage(code) {
    const messages = {
      empty_text: "Write a note first.",
      too_many_lines: "Maximum 10 lines.",
      text_too_long: "Notes can be up to 250 characters.",
      invalid_json: "Could not read this note.",
      invalid_date: "Choose a valid date.",
      history_not_available: "History starts on 2026-10-14.",
      future_date: "Future dates are unavailable.",
      admin_required: "Admin cookie required.",
      origin_required: "Admin actions must come from this site.",
      rate_limited: "Too many posts. Try again in a moment.",
      internal_error: "Something went wrong. Try again."
    };
    return messages[code] || "Could not complete that request. Try again.";
  }

  async function api(path, options = {}) {
    const headers = { ...(options.headers || {}) };
    if (options.body) headers["content-type"] = "application/json";

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

  function renderNotes(notes) {
    cardResizeObserver.disconnect();
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

      const content = document.createElement("p");
      content.textContent = note.text;
      contentViewport.append(content);

      const time = document.createElement("time");
      time.dateTime = note.created_at;
      time.textContent = formatTime(note.created_at);

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
      cardResizeObserver.observe(card);
    }
  }

  function setSelectedDate(date, notes) {
    state.selectedDate = date;
    state.notes = notes;
    dateInput.value = date;
    boardHeading.textContent = date;
    noteForm.hidden = date !== state.today;
    boardStatus.textContent = date === state.today
      ? (state.today >= HISTORY_START_DATE ? "Today" : errorMessage("history_not_available"))
      : "Archive · Read only";
    renderNotes(notes);
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
    if (state.selectedDate) renderNotes(state.notes);
  }

  async function deleteNote(id, button) {
    button.disabled = true;
    try {
      await api(`/api/admin/notes/${encodeURIComponent(id)}`, { method: "DELETE" });
      if (state.selectedDate === state.today) await loadToday();
      else await loadDate(state.selectedDate);
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
      dateInput.min = HISTORY_START_DATE;
      dateInput.max = data.date;
      dateInput.disabled = data.date < HISTORY_START_DATE;
      viewDateButton.disabled = data.date < HISTORY_START_DATE;
      setSelectedDate(data.date, data.notes);
      formStatus.textContent = "";
    } catch (error) {
      boardStatus.textContent = "Offline";
      notesContainer.replaceChildren();
      showEmptyError(error);
    } finally {
      notesContainer.setAttribute("aria-busy", "false");
    }
  }

  async function loadDate(date) {
    if (!date) {
      boardStatus.textContent = "Choose a date";
      return;
    }
    if (state.today && date < HISTORY_START_DATE) {
      dateInput.value = state.today >= HISTORY_START_DATE ? state.today : "";
      boardStatus.textContent = errorMessage("history_not_available");
      return;
    }
    if (state.today && date > state.today) {
      dateInput.value = state.today;
      boardStatus.textContent = errorMessage("future_date");
      return;
    }
    boardStatus.textContent = "Loading…";
    notesContainer.setAttribute("aria-busy", "true");
    try {
      const data = await api(`/api/notes?date=${encodeURIComponent(date)}`);
      setSelectedDate(data.date, data.notes);
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

  // Mirrors the Worker: drop leading blank lines and trailing whitespace, keep indentation.
  function trimNoteText(value) {
    return value.trimEnd().replace(/^(?:[^\S\r\n]*(?:\r\n|\r|\n))+/, "");
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

  viewDateButton.addEventListener("click", () => loadDate(dateInput.value));
  viewTodayButton.addEventListener("click", loadToday);
  resetTodayButton.addEventListener("click", resetToday);
  dateInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      loadDate(dateInput.value);
    }
  });

  noteText.addEventListener("input", enforceLineLimit);
  noteText.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      if (!submitButton.disabled) noteForm.requestSubmit();
    }
  });

  noteForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const text = trimNoteText(noteText.value);
    if (!text) {
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
      await api("/api/notes", {
        method: "POST",
        body: JSON.stringify({ text })
      });
      noteText.value = "";
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
    if (!document.hidden && state.selectedDate === state.today) loadToday();
  });

  updateCharacterCount();
  refreshAdminSession().then(loadToday);
})();
