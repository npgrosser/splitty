import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeSocket } from "../testing/fake_server.ts";
import { ViewerReviewError, ViewerReviews, utf16Offsets } from "./server_reviews.ts";

const encoder = new TextEncoder();
const bytesBefore = (text: string, at: number) => encoder.encode(text.slice(0, at)).length;

describe("utf16Offsets", () => {
  // mux-protocol, Review Texts And Anchors: Ranges in the source.
  it("turns UTF-8 byte offsets of the source text into offsets of the window's text", () => {
    const text = "Größe 😀 der Änderung\nnext line";
    const quote = text.indexOf("der");
    const after = text.indexOf("next");
    expect(utf16Offsets(text, [bytesBefore(text, after), 0, bytesBefore(text, quote), encoder.encode(text).length]))
      .toEqual([after, 0, quote, text.length]);
  });

  it("names no offset inside a character or past the text", () => {
    const text = "a😀b";
    expect(utf16Offsets(text, [2, 5, 6, 7])).toEqual([undefined, 3, 4, undefined]);
  });
});

describe("ViewerReviews", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks for the anchors of a pane and takes the server's answer to that request", async () => {
    const socket = new FakeSocket();
    const reviews = new ViewerReviews(socket);
    const anchored = reviews.anchor("v1");
    const [sent] = socket.sentOfType("viewer.anchor");
    expect(sent).toMatchObject({ type: "viewer.anchor", paneId: "v1" });
    socket.message({ type: "viewer.anchored", requestId: "vq-other", paneId: "v1", version: "x", anchors: [] });
    socket.message({
      type: "viewer.anchored", requestId: sent.requestId, paneId: "v1", version: "7",
      anchors: [{ id: "a1", state: "general" }, { id: "a2", state: "changed", start: 4, end: 9, startLine: 2, endLine: 3 }],
    });
    await expect(anchored).resolves.toEqual({
      paneId: "v1", version: "7",
      anchors: [{ id: "a1", state: "general" }, { id: "a2", state: "changed", start: 4, end: 9, startLine: 2, endLine: 3 }],
    });
    reviews.dispose();
  });

  it("refuses anchors that do not keep to the protocol", async () => {
    const socket = new FakeSocket();
    const reviews = new ViewerReviews(socket);
    const anchored = reviews.anchor("v1");
    const [sent] = socket.sentOfType("viewer.anchor");
    socket.message({ type: "viewer.anchored", requestId: sent.requestId, version: "7", anchors: [{ id: "a1", state: "anchored", start: 9, end: 4, startLine: 1, endLine: 1 }] });
    await expect(anchored).rejects.toBeInstanceOf(ViewerReviewError);
    reviews.dispose();
  });

  // mux-protocol, Review Texts And Anchors: A review on request, Passages of a rendered view.
  it("asks for a review with the verdict and the rendered view's passages, and takes its text and entries", async () => {
    const socket = new FakeSocket();
    const reviews = new ViewerReviews(socket);
    const reviewed = reviews.review({
      paneId: "v1", verdict: "changes_requested", changedDuringWait: true,
      passages: [{ id: "a3", state: "anchored", startLine: 4, endLine: 4, offset: 12 }],
    });
    const [sent] = socket.sentOfType("viewer.review");
    expect(sent).toMatchObject({
      type: "viewer.review", paneId: "v1", verdict: "changes_requested", changedDuringWait: true,
      passages: [{ id: "a3", state: "anchored", startLine: 4, endLine: 4, offset: 12 }],
    });
    const entries = [{ id: "a3", kind: "comment", body: "Why?" }];
    socket.message({ type: "viewer.reviewed", requestId: sent.requestId, paneId: "v1", version: "7", text: "# Review of PLAN.md\n", count: 1, entries });
    await expect(reviewed).resolves.toEqual({ paneId: "v1", version: "7", text: "# Review of PLAN.md\n", count: 1, entries });

    const plain = reviews.review({ paneId: "v1", passages: [] });
    expect(socket.sentOfType("viewer.review")[1]).toEqual({ type: "viewer.review", paneId: "v1", requestId: socket.sentOfType("viewer.review")[1].requestId });
    socket.message({ type: "viewer.reviewed", requestId: socket.sentOfType("viewer.review")[1].requestId, paneId: "v1", version: "7", count: 0, entries: [] });
    await expect(plain).rejects.toBeInstanceOf(ViewerReviewError);
    reviews.dispose();
  });

  // mux-protocol, Review Texts And Anchors: A pane that is gone.
  it("fails with the server's reason, when the connection ends, and after a while without an answer", async () => {
    vi.useFakeTimers();
    const socket = new FakeSocket();
    const reviews = new ViewerReviews(socket, 1000);
    const gone = reviews.review({ paneId: "v9" });
    socket.message({ type: "viewer.failed", requestId: socket.sentOfType("viewer.review")[0].requestId, code: "unknown_pane", error: "No such pane\u001b[2J" });
    await expect(gone).rejects.toMatchObject({ code: "unknown_pane" });

    const dropped = reviews.anchor("v1");
    socket.drop();
    await expect(dropped).rejects.toBeInstanceOf(ViewerReviewError);

    const late = reviews.anchor("v1");
    vi.advanceTimersByTime(1000);
    await expect(late).rejects.toThrow("in time");
    reviews.dispose();
  });
});
