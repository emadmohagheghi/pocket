// Tests for the shared voice player logic in store.ts — the area that shipped
// two real bugs (NaN-duration seeks, delete-while-playing kept playing), so it
// gets regression coverage.
//
// The store imports api (Tauri invoke wrappers) at module scope, so api is
// mocked wholesale; every action under test here is synchronous state logic
// and never calls the backend.

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api", () => ({
  api: Object.fromEntries(
    "getState convertHtmlToMarkdown frontendReady markWhatsNewSeen getItems createWorkspace renameWorkspace deleteWorkspace getWorkspaceCounts setActiveWorkspace createItem updateItem deleteItem moveItem deleteEntriesBulk setPinned search saveRecording saveImage renameRecording deleteRecording copyToClipboard openUrl applyUndo updateSettings getGamingState getStorageInfo openDataFolder closeWindow exportBackup importBackup openVoiceCapture installAndLaunchUpdate log"
      .split(" ")
      .map((name) => [name, vi.fn()])
  ),
  voiceUrl: vi.fn(() => ""),
  imageUrl: vi.fn(() => ""),
  extFromMimeType: vi.fn(() => "png"),
  recordingExtension: vi.fn(() => "audio/webm;codecs=opus"),
}));

const storeModule = await import("@/store");
const { usePocket, registerAudioEngineElement, stopPlayerIfGoneForTest } = storeModule;
import type { Item, Recording, WorkspaceData } from "@/types";

function makeRecording(id: string, durationMs = 8000): Recording {
  return {
    id,
    name: `voice-${id}`,
    file: `${id}.webm`,
    durationMs,
    sizeBytes: 1000,
    pinned: false,
    createdAt: 0,
  };
}

function makeWorkspaceData(recordings: Recording[], items: Item[] = []): WorkspaceData {
  return { items, recordings };
}

function makeItem(id: string, recording: Recording | null): Item {
  return {
    id,
    itemType: "text",
    content: "",
    title: null,
    url: null,
    images: [],
    recording,
    pinned: false,
    createdAt: 0,
    updatedAt: 0,
  };
}

/** Reset all store fields the player tests touch; keep the store's own
    defaults for everything else. */
function resetPlayerState() {
  usePocket.setState({
    ready: true,
    settings: {
      launchOnStartup: false,
      startMinimized: false,
      activeWorkspaceId: "ws-1",
      gamingDetectionEnabled: false,
      alwaysOnTop: false,
      theme: "system",
      notePreviewLines: 0,
      lastLaunchVersion: null,
      whatsNewSeenVersion: null,
    },
    workspaces: [
      { id: "ws-1", name: "Main", createdAt: 0, updatedAt: 0, counts: { texts: 0, recordings: 0 } },
      { id: "ws-2", name: "Other", createdAt: 0, updatedAt: 0, counts: { texts: 0, recordings: 0 } },
    ],
    data: makeWorkspaceData([]),
    player: null,
    playerPlaying: false,
    playerTime: 0,
    playerDuration: 0,
    playerSeekRequest: null,
    playerScrubbing: false,
  });
}

describe("voice player store logic", () => {
  beforeEach(() => {
    resetPlayerState();
    registerAudioEngineElement(null);
  });

  describe("playRecording", () => {
    it("starts a track playing with duration derived from recording metadata", () => {
      usePocket.getState().playRecording(makeRecording("r1", 8000));

      const s = usePocket.getState();
      expect(s.player?.recordingId).toBe("r1");
      expect(s.player?.wsId).toBe("ws-1");
      expect(s.player?.duration).toBe(8);
      expect(s.playerPlaying).toBe(true);
      expect(s.playerTime).toBe(0);
      expect(s.playerDuration).toBe(8);
    });
  });

  describe("togglePlayer", () => {
    it("pauses and resumes a normal (mid-track) playback", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().togglePlayer();
      expect(usePocket.getState().playerPlaying).toBe(false);
      usePocket.getState().togglePlayer();
      expect(usePocket.getState().playerPlaying).toBe(true);
    });

    it("does nothing without an active track", () => {
      usePocket.getState().togglePlayer();
      expect(usePocket.getState().player).toBeNull();
      expect(usePocket.getState().playerPlaying).toBe(false);
    });

    it("restarts from the beginning when playback ended", () => {
      usePocket.getState().playRecording(makeRecording("r1", 8000));
      // Natural end of track, engine element confirms `ended`.
      usePocket.getState().reportPlayerProgress(8, 8, false);
      registerAudioEngineElement({ ended: true, currentTime: 8 } as unknown as HTMLAudioElement);

      usePocket.getState().togglePlayer();
      const s = usePocket.getState();
      expect(s.playerPlaying).toBe(true);
      expect(s.playerSeekRequest).toBe(0);
    });

    it("does not treat a mid-scrub position near the end as ended", () => {
      usePocket.getState().playRecording(makeRecording("r1", 8000));
      // User is scrubbing near the end of the track.
      usePocket.getState().setPlayerScrubbing(true);
      usePocket.getState().requestPlayerSeek(7.8);
      usePocket.getState().togglePlayer();
      // Plain pause, NOT a restart-to-zero: the scrub's own pending seek
      // (7.8) is preserved, no seek to 0 is injected.
      const s = usePocket.getState();
      expect(s.playerSeekRequest).toBe(7.8);
      expect(s.playerPlaying).toBe(false);
    });
  });

  describe("requestPlayerSeek", () => {
    it("sets an optimistic time and a seek request", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().requestPlayerSeek(3.5);
      const s = usePocket.getState();
      expect(s.playerSeekRequest).toBe(3.5);
      expect(s.playerTime).toBe(3.5);
    });

    it("clamps negative targets to zero", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().requestPlayerSeek(-2);
      expect(usePocket.getState().playerSeekRequest).toBe(0);
    });

    it("is ignored without an active track", () => {
      usePocket.getState().requestPlayerSeek(3.5);
      expect(usePocket.getState().playerSeekRequest).toBeNull();
    });
  });

  describe("reportPlayerProgress", () => {
    it("accepts element reports while idle", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().reportPlayerProgress(2, 8, true);
      const s = usePocket.getState();
      expect(s.playerTime).toBe(2);
      expect(s.playerDuration).toBe(8);
      expect(s.playerPlaying).toBe(true);
    });

    it("ignores element reports during an active waveform drag", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().requestPlayerSeek(4.2);
      usePocket.getState().setPlayerScrubbing(true);
      // Audio element still at pre-seek position reports stale progress.
      usePocket.getState().reportPlayerProgress(0.3, 8, true);
      expect(usePocket.getState().playerTime).toBe(4.2);
    });

    it("resumes accepting reports once the drag ends", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().setPlayerScrubbing(true);
      usePocket.getState().requestPlayerSeek(4.2);
      usePocket.getState().reportPlayerProgress(0.3, 8, true);
      usePocket.getState().setPlayerScrubbing(false);
      usePocket.getState().reportPlayerProgress(4.2, 8, true);
      expect(usePocket.getState().playerTime).toBe(4.2);
    });
  });

  describe("stopPlayer", () => {
    it("clears all player state", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.getState().stopPlayer();
      const s = usePocket.getState();
      expect(s.player).toBeNull();
      expect(s.playerPlaying).toBe(false);
      expect(s.playerTime).toBe(0);
      expect(s.playerDuration).toBe(0);
      expect(s.playerSeekRequest).toBeNull();
    });
  });

  describe("delete-while-playing guard (items-changed path)", () => {
    it("stops playback when the playing recording is deleted", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      // Backend reports fresh workspace data without the playing track.
      usePocket.setState({ data: makeWorkspaceData([]) });

      stopPlayerIfGoneForTest(usePocket.getState(), "ws-1");
      expect(usePocket.getState().player).toBeNull();
    });

    it("keeps playback when the playing recording still exists", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.setState({ data: makeWorkspaceData([makeRecording("r1")]) });

      stopPlayerIfGoneForTest(usePocket.getState(), "ws-1");
      expect(usePocket.getState().player?.recordingId).toBe("r1");
    });

    it("keeps playback while a note still embeds the recording (recordings list lags)", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      // Not in recordings[], but a note still embeds it → it still exists.
      usePocket.setState({
        data: makeWorkspaceData([], [makeItem("i1", makeRecording("r1"))]),
      });

      stopPlayerIfGoneForTest(usePocket.getState(), "ws-1");
      expect(usePocket.getState().player?.recordingId).toBe("r1");
    });

    it("stops playback when the track is gone from both recordings and items", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      // Note deleted: neither the recording nor an embedding item remains
      // (another recording-less note exists to exercise the items scan).
      usePocket.setState({
        data: makeWorkspaceData([], [makeItem("i2", null)]),
      });

      stopPlayerIfGoneForTest(usePocket.getState(), "ws-1");
      expect(usePocket.getState().player).toBeNull();
    });

    it("ignores another workspace's data update (playback continues)", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      // Playing track belongs to ws-2; incoming data is ws-1's.
      usePocket.setState({ player: { ...usePocket.getState().player!, wsId: "ws-2" } });
      usePocket.setState({ data: makeWorkspaceData([]) });

      stopPlayerIfGoneForTest(usePocket.getState(), "ws-1");
      expect(usePocket.getState().player?.recordingId).toBe("r1");
    });
  });

  describe("workspace guard (state-changed path)", () => {
    it("stops playback when the track's workspace no longer exists", () => {
      usePocket.getState().playRecording(makeRecording("r1"));
      usePocket.setState({ player: { ...usePocket.getState().player!, wsId: "ws-gone" } });

      stopPlayerIfGoneForTest(usePocket.getState());
      expect(usePocket.getState().player).toBeNull();
    });

    it("keeps playback while the track's workspace exists", () => {
      usePocket.getState().playRecording(makeRecording("r1"));

      stopPlayerIfGoneForTest(usePocket.getState());
      expect(usePocket.getState().player?.recordingId).toBe("r1");
    });
  });
});
