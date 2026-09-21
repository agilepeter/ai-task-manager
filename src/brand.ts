// Who made this build, and where it points. THIS IS THE FILE A FORK EDITS:
// everything the About panel says about its maker comes from here, so
// re-branding is one file, not a hunt. The credits below it are not yours to
// remove: they are the MIT notices of the projects this one is built on.

export const BRAND = {
  product: "AI Task Manager",
  tagline: "Everything your AI tools are doing, in one place.",
  maker: "StaaS Fund",
  makerUrl: "https://staas.fund/",
  by: "Peter Saddington",
  links: [
    { label: "The Library", hint: "Guides, playbooks and explainers", url: "https://staas.fund/library/" },
    { label: "MCP Trust Index", hint: "Which MCP servers are worth trusting", url: "https://staas.fund/mcp/" },
    { label: "The Open Classroom", hint: "Learn the ideas this app points at", url: "https://staas.fund/classroom/" },
    { label: "Build Anything workshop", hint: "Learn to build with AI, in a room", url: "https://staas.fund/workshop/" },
  ],
} as const;

/** Upstream credit. Keep this in any fork: it is the licence, not decoration. */
export const CREDITS = [
  { name: "Pane", by: "Jazii", url: "https://github.com/ItsJazii/pane", note: "the Windows tray app this grew from" },
  { name: "OpenUsage", by: "Robin Ebers", url: "https://github.com/robinebers/openusage", note: "the macOS original and the provider research" },
] as const;
