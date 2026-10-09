# Notes on the timeline

A design, not yet built past what "Where it stands" describes.  It
follows a review of the notes as they are (by hand, and by a design
review that used the pages as an operator would), and Grafana's
annotations, which the range control already copies.

## What a note is for

The receiver cannot report what happens around it: a part swapped, an
antenna moved, the daemon stopped for a test, the room's sensor left
in the sun.  A note puts that on the same timeline as the readings, so
a step in the EFC can be told from a step on the bench.  It is a
record: once written it is not lost, which is why a change keeps what
the note said before (`note_change`, schema 13).

## Where it stands

- A note is a moment and text, in one receiver's log, written through
  the daemon attached to it (the log's only writer): by
  `smartclock-cli note`, or from the web view's journal.  It can be
  changed or deleted by its id; a change made over a stale copy, a
  note dated in the future, and another receiver's note are refused.
- The charts draw each note as a dashed line through every chart, its
  text shown when the pointer is within a few pixels of it.  Clicking
  a note in the journal centers the charts an hour either side.
- The journal's notes tab adds a note "now", or at a time typed in or
  picked by clicking a chart, and edits or deletes one in its row.
- Times are shown in the viewer's zone on a 12- or 24-hour clock, or
  in UTC (`tz=`); the address carries UTC only.

What does not work well, most important first:

1. **Adding a note at a moment on a chart is a round trip.**  The form
   is below the charts, about a screen away; "pick on chart" means
   scrolling up, clicking, and scrolling back to Add.
2. **A marker cannot be told apart or read at a glance.**  It is a
   faint dashed line like the cursor's, in a green close to a series',
   with no label, readable only by hovering within a few pixels.
3. **Notes at one moment hide each other:** only the first can be read
   from a chart.
4. **The note jumped to is not marked** among the others.
5. **A phone cannot read a marker** (there is no hover), and the form
   and the edit row do not fit.
6. **Messages are in the wrong place and stay:** an edit's refusal shows
   in the add form, and "added" lingers.  An armed delete looks like
   the button it replaced.
7. **Times are written three ways:** journal, axis and range control.
8. **Bench events that last are forced into moments:** "stick in the
   sun 16:35-18:10" is two notes or one with the end in its text, and
   nothing can act on the span.
9. **A note about the whole bench** is written to each daemon by hand,
   and each copy is changed separately.

## The model

A note is a mark on the timeline: a moment or a span, about one
receiver or the whole bench, written by a person or recorded by the
system.

| Kind | Example | Drawn as |
| ---- | ------- | -------- |
| Point | "swapped the splitter" | a line through every chart |
| Span | "stick in the sun 16:35-18:10" | a band across every chart |
| Bench | "replaced the splitter feeding all three" | as above, on every receiver |
| Automatic | daemon started or upgraded, receiver powered up, holdover, reattached | gray, not editable |

**Tags** are free words on a note, as in Grafana.  One is understood:
`exclude` on a span leaves it out of what the stability and
correlation pages compute -- the crystal trim, the sun on the stick --
and those pages say so beside their figures ("2 spans left out").

**Automatic marks** come from what the logs already hold -- the
receiver's power-on entries, holdover, a reattach -- and from one new
record, the daemon's own start: its version, the one before if it
changed, and whether the host had rebooted.  Starts in quick
succession, a crash loop, are one mark with a count, so it is never
busy.

## On the charts

As Grafana draws annotations, with what it lacks for a bench added.

- **A marker strip** along the bottom of the stack, by the time axis
  where the eye already is.  A point is a flag there and a solid
  hairline up through every chart; a span is a bar there and a faint
  band across every chart.  Notes have a color of their own that no
  series or state uses; automatic marks are gray.  Flags carry a
  number, 1, 2, 3 in time order within the view, and the journal shows
  the same numbers, so a mark and its row are matched by eye.
- **Density.**  Flags closer than a flag's width merge into one, "3+2".
  At a week or a month the strip becomes ticks, and a mark's line is
  drawn only while it is hovered, so the plots stay readable.
- **Reading.**  Hovering a flag, or a line anywhere along it, opens a
  card listing every note there: time, span, text, tags, and edit and
  delete.  The readout row keeps naming the note under the pointer.
- **Selecting.**  Clicking a flag selects its note: its line or band
  is drawn brighter, its row scrolls into view, and the editor opens.
  A note clicked in the journal is selected the same way.
- **Adding,** as in Grafana: Ctrl-click (Cmd on a Mac) on any chart
  adds a point there, Ctrl-drag a span.  A plain drag still zooms.  `n`
  adds a point at the pointer.  The strip says so ("Ctrl-click to add
  a note"), since a modifier key is not found by accident.
- **Layers.**  The people's notes, bench notes and automatic marks are
  each shown or hidden, on every page with time charts: Live, Compare,
  Correlation.

## One editor

The same editor adds and changes a note, wherever it opens: at a
moment on a chart, at a flag, or in a journal row.

- **Text,** several lines.  Ctrl-Enter saves, Esc cancels.
- **When:** the start, and for a span the end, to the second, in the
  zone and clock the page shows.  "now" until changed; one-minute
  nudges; "pick on chart" for either end.
- **Scope:** this receiver or the whole bench.  **Tags,** with
  `exclude` offered for a span.
- **Save, cancel and delete,** delete taking a second click and colored
  as a warning while armed.  Messages appear in the editor, and clear.
- **History:** "edited 2 times" opens what the note said before, from
  `note_change`.
- The page holds off reading again while it is open, as it does now.

## The notes list

The journal's notes tab becomes a list: number, time, duration, text,
tags, scope, in one time format with the rest of the page.  A filter
box, and "in view only", on when the range is fixed.  Automatic marks
are listed, grayed.  A row selects its note and centers the charts on
it.

## On a phone

No hover, no modifier keys: touch instead.

- **Reading the charts:** press and hold to show the crosshair and the
  readout; drag to scrub; lift and the readout stays until a tap
  elsewhere.  Double tap zooms out.
- **Markers:** flags at least 32 px tall.  Tapping one opens a panel
  from the bottom listing the notes there; tapping a note there
  selects it, with edit and delete.
- **Adding:** a "+ note" button at the bottom right, dated now.  A long
  press on a chart opens the editor at that moment.  "pick on chart"
  in the editor shrinks it to a bar until a moment is tapped.
- **The editor** is the panel from the bottom, full width, kept above
  the keyboard, with the phone's own date and time wheels.
- **The list:** two lines a row, time and duration over text.
- **The rest of the page:** the status strip folds to one line; the
  range control scrolls sideways rather than wrapping.

Grafana documents no touch interaction for annotations, so this part
is ours.

## Elsewhere

- **CLI:** `note --until TIME` for a span, `--bench`, `--tag TAG`;
  `notes` lists them with their numbers.
- **Terminal monitor:** spans drawn in its history charts; the journal
  lists every kind.
- **Exporter:** nothing; Prometheus has no annotations.

## What it takes

- **Schema 14:** `note` gains `until` (the end, NULL for a point),
  `tags`, and `bench` (an id shared by every daemon's copy of a bench
  note).  `note_change` records them as it records the text.  A table
  for the daemon's starts.  All by the daemon's `migrate()`, tested.
- **Ops:** `note` and `note_edit` take the new fields; `notes` lists.
  A bench note is written to every daemon; the page says which took
  it, and offers to retry the rest.
- **The web view** gains the strip, the card, the editor and the list
  as shared pieces, used by every chart page, rather than one page's
  code.

## Adversarial review

- **A modifier key to add, a plain drag to zoom.**  Grafana's users
  know it; nobody else finds it alone.  Hence the hint in the strip,
  `n`, and the journal's "+".
- **A bench note lives in several logs.**  A daemon down when it is
  written misses it.  The page says so and retries on request; the
  shared id makes a later change reach every copy that exists.
- **`exclude` changes results without being seen** unless every page
  that honors it says so beside the figure.  It applies to spans only.
- **Automatic marks could crowd the people's** at a long range.  They
  are a separate layer, gray, merged when close, and off by default
  beyond a week.
- **Size.**  The largest piece of web work so far, so it is built in
  phases, each usable alone.

## Phases

1. **Fixes:** a color of its own and solid lines; every note at a
   moment readable; messages where they belong, cleared; one time
   format, to the second; the selected note marked; the phone layout.
2. **On the chart:** the strip with numbered flags and the card; the
   one editor; Ctrl-click and `n` to add.
3. **Spans,** Ctrl-drag, and `exclude` honored by the stability and
   correlation pages (schema 14).
4. **Bench notes.**
5. **Automatic marks,** the daemon's starts among them.
6. **The phone,** throughout.

## Open questions

1. The marker strip at the bottom of the stack, as Grafana places it,
   or at the top?
2. Free tags, with `exclude` the one that means something, or a fixed
   "leave out of analysis" switch and no tags?
3. A color of the notes' own: which?
4. `n` and Ctrl-click on this page: welcome?
5. The phone in phase 2 alongside the desktop, or after it?
6. Notes of several lines, or one line kept short?
7. Is showing a note's earlier versions in the editor worth it?
8. Should dragging a note's line or band on a chart move its time, or
   is that too easy to do by accident?
9. Automatic marks: which starts -- the daemon's, the host's, the
   receiver's power-ups -- and on the charts or in the journal only?
10. On a phone, a floating "+ note" button, or adding only from the
    notes tab?
11. Press-and-hold to read a chart on a phone: on every chart page, or
    Live only?
12. Delete by a second click, as now, or remove at once with a few
    seconds to undo?  The design keeps the second click: an undo held
    in the page is lost if the tab is closed.
