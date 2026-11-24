import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { RobotToolResult } from "hebbian-rua";
import { afterEach, expect, test, vi } from "vitest";
import { ClaudeConversation } from "../src/providers/claude.js";
import { GeminiConversation } from "../src/providers/gemini.js";
import { OpenAIConversation } from "../src/providers/openai.js";
import {
  type Observation,
  PROVIDERS,
  type Provider,
} from "../src/providers/types.js";

const testModelIds = {
  anthropic: "claude-fable-5-2",
  openai: "gpt-6.2-astra",
  gemini: "gemini-4-neon",
};

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

type ImageBlock = {
  type: string;
  source?: { data: string };
  image_url?: string;
  data?: string;
};

type RequestWithImages = {
  messages?: { role: string; content: ImageBlock[] }[];
  input?: { content?: ImageBlock[] }[];
};

function submittedImages(
  provider: Provider,
  request: RequestWithImages,
): Buffer[] {
  const blocks =
    provider === "anthropic"
      ? request.messages?.findLast((message) => message.role === "user")
          ?.content
      : request.input?.flatMap((item) => item.content ?? []);
  return (blocks ?? []).flatMap((block) => {
    const encoded =
      block.source?.data ??
      block.image_url?.split(",")[1] ??
      (block.type === "image" ? block.data : undefined);
    return encoded === undefined ? [] : [Buffer.from(encoded, "base64")];
  });
}

function modelReply(provider: Provider, turn: number): Response {
  switch (provider) {
    case "openai":
      return Response.json({
        id: `response-${turn}`,
        status: "completed",
        output: [],
      });
    case "gemini":
      return Response.json({
        id: `interaction-${turn}`,
        status: "completed",
        steps: [],
      });
    case "anthropic": {
      const events = [
        {
          type: "message_start",
          message: {
            id: `message-${turn}`,
            type: "message",
            role: "assistant",
            content: [],
            model: testModelIds.anthropic,
            stop_reason: null,
            stop_sequence: null,
            usage: { input_tokens: 1, output_tokens: 0 },
          },
        },
        {
          type: "message_delta",
          delta: { stop_reason: "end_turn", stop_sequence: null },
          usage: { output_tokens: 1 },
        },
        { type: "message_stop" },
      ];
      return new Response(
        events
          .map(
            (event) =>
              `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`,
          )
          .join(""),
        { headers: { "Content-Type": "text/event-stream" } },
      );
    }
  }
}

function observation(cameras: Record<string, number[]>): Observation {
  return {
    text: "Station observation",
    cameras: Object.fromEntries(
      Object.entries(cameras).map(([name, bytes]) => [
        name,
        Buffer.from(bytes),
      ]),
    ),
  };
}

function observationResult(
  id: string,
  cameras: Record<string, number[]>,
): RobotToolResult {
  return {
    call: { id, name: "observe", arguments: {} },
    text: "Camera observations",
    data: null,
    modelData: null,
    isError: false,
    halt: false,
    interrupted: false,
    cameras: observation(cameras).cameras,
  };
}

test.each(PROVIDERS)(
  "%s sends the reference image and archives camera images in request order",
  async (provider) => {
    const directory = await mkdtemp(join(tmpdir(), "rua-model-turns-"));
    vi.stubEnv("HEBBIAN_RUA_MODEL_INPUTS_DIR", directory);
    vi.stubEnv("ANTHROPIC_BASE_URL", "http://model.test");
    vi.stubEnv("OPENAI_BASE_URL", "http://model.test");
    vi.stubEnv("GOOGLE_GEMINI_BASE_URL", "http://model.test");
    const requests: RequestWithImages[] = [];
    vi.stubGlobal(
      "fetch",
      async (input: string | URL | Request, init?: RequestInit) => {
        const request = new Request(input, init);
        expect(new URL(request.url).host).toBe("model.test");
        requests.push((await request.json()) as RequestWithImages);
        return modelReply(provider, requests.length);
      },
    );
    const Conversation = {
      anthropic: ClaudeConversation,
      openai: OpenAIConversation,
      gemini: GeminiConversation,
    }[provider];

    try {
      const conversation = new Conversation({
        apiKey: "local-test-key",
        model: testModelIds[provider],
        systemPrompt: "Inspect the station",
        firstObservation: {
          ...observation({ wrist: [1, 2] }),
          referenceImageJpeg: Buffer.from([13, 14]),
        },
        tools: [],
        reasoningEffort: "high",
      });
      await conversation.nextTurn();
      conversation.addToolResults(
        [
          observationResult("inspect-left", { left: [3, 4], right: [5, 6] }),
          observationResult("inspect-right", { wrist: [7, 8] }),
        ],
        observation({ overhead: [9, 10] }),
      );
      await conversation.nextTurn();
      conversation.addToolResults([observationResult("no-images", {})]);
      await conversation.nextTurn();
      conversation.addObservation(observation({ right: [11, 12] }));
      await conversation.nextTurn();

      const [run, ...others] = await readdir(directory);
      expect(others).toEqual([]);
      if (!run) throw new Error("Missing model input run directory");
      const runPath = join(directory, run);
      expect((await readdir(runPath)).sort()).toEqual([
        "turn-0001",
        "turn-0002",
        "turn-0004",
      ]);

      const expectedImages = [
        [{ file: "1.jpg", camera: "wrist" }],
        [
          { file: "1.jpg", camera: "left", tool_call_id: "inspect-left" },
          { file: "2.jpg", camera: "right", tool_call_id: "inspect-left" },
          { file: "3.jpg", camera: "wrist", tool_call_id: "inspect-right" },
          { file: "4.jpg", camera: "overhead" },
        ],
        [],
        [{ file: "1.jpg", camera: "right" }],
      ];
      expect(requests).toHaveLength(expectedImages.length);
      for (const [index, images] of expectedImages.entries()) {
        const turn = index + 1;
        const request = requests[index];
        if (!request) throw new Error(`Missing request for model turn ${turn}`);
        const sent = submittedImages(provider, request);
        if (index === 0) expect(sent.pop()).toEqual(Buffer.from([13, 14]));
        if (images.length === 0) {
          expect(sent).toEqual([]);
          continue;
        }
        const turnPath = join(runPath, `turn-000${turn}`);
        expect(
          JSON.parse(await readFile(join(turnPath, "index.json"), "utf8")),
        ).toEqual({
          turn,
          images,
        });
        const saved = await Promise.all(
          images.map(({ file }) => readFile(join(turnPath, file))),
        );
        expect(saved).toEqual(sent);
        expect(saved.map((image) => image[0])).toEqual(
          [[1], [3, 5, 7, 9], [], [11]][index],
        );
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  },
);
