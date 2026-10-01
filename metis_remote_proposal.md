# Project Proposal: Metis RUDP-Stream (Native Low-Latency Remote Desktop Protocol)

## Context & Vision
We are designing a next-generation, ultra-low-latency remote desktop streaming engine integrated directly into **Metis Desktop** (a custom Wayland Desktop Environment built in **Rust** using the **Smithay** compositor framework). 

Unlike traditional third-party remote desktop tools (Parsec, Sunshine, FreeRDP) that act as external layers scraping the OS, this solution will be **deeply embedded within the compositor's architecture**. It operates entirely **peer-to-peer (P2P)** with no corporate middleman or coordination server, targeting standard direct connection setups (`IP:Port`).

The goal is to outperform every existing tool in performance, latency, and WAN bandwidth efficiency, making it fully capable of handling standard text work, high-end 3D CAD/Blender viewports, and fast-paced gaming.

---

## 🛠 Architectural Pillars

### 1. Compositor-Level Frame Capture (Zero-Copy Pipeline)
*   **The Blueprint:** Completely bypass PipeWire, Portal, and X11 interfaces. Intercept frames directly inside Smithay's inner rendering loop (e.g., `GlesRenderer`).
*   **Implementation:** Export native hardware texture pointers (`EGLImage` or Vulkan `VkImage` memory handles) as a `DMA-BUF` file descriptor. Pass this fd directly to the hardware encoder (Vulkan/VAAPI or NVENC wrappers via `ash` or `ffmpeg-next`).
*   **Advantage:** Frames never cross user-space or touch the CPU. Sub-millisecond host capture.

### 2. The Network Stack: Custom RUDP Over QUIC
*   **The Blueprint:** Use pure-Rust `quinn` to establish an mTLS/cryptographically secured UDP/QUIC connection.
*   **Reliable vs. Unreliable:** Handshakes and input configuration use reliable streams. Raw video packet payloads use QUIC's *unreliable, unordered datagram channels* to completely eliminate TCP head-of-line blocking.
*   **Predictive Forward Error Correction (FEC):** Integrate `reed-solomon-erasure`. Inject dynamic parity shards on the host side based on real-time network conditions. The client reconstructs missing packets instantly. No RTT stalls on packet drops.

### 3. Native Input Injection
*   **The Blueprint:** Capture raw client input via `libevdev` and stream as lightweight 64-bit UDP packets.
*   **Host-Side Handling:** Bypass kernel-level loopbacks (`/dev/uinput`). Pipe the network stream directly into Smithay’s internal `InputState` engine. 
*   **Advantage:** Processes network inputs as natively and quickly as local hardware interrupts. Support Wayland Relative Pointer protocols natively for 3D workspaces and gaming.

### 4. Smart Compression & WAN Optimization
*   **Wayland Damage Tracking:** Leverage Smithay's native coordinate dirty-tracking. When the screen is mostly static, only encode and transmit the modified bounding boxes. Bandwidth drops to near zero for office tasks.
*   **High-Motion Auto-Switching:** Dynamically pivot to Low-Latency Constant Bitrate (CBR) and engage BBR v3 congestion control if screen alterations span >70% of the display (e.g., rotating viewport in Blender).
*   **Text & Asset Clarity:** Enforce H.265 or AV1 hardware profiles with **YUV 4:4:4** chroma subsampling (or 10-bit P010 color profiles) to maintain razor-sharp text and wireframes without dark-gradient color banding.

---

## 🤖 Prompt for AI / Cursor Task Execution

Hello Cursor. I want you to read the architectural proposal above for the Metis RUDP-Stream framework. We are going to design this natively inside our Rust/Smithay environment. 

Please perform a rigorous code-readiness audit on this proposal and help me lay down the foundation by answering the following:

1. **Smithay Texture Export Integration:** Identify exactly where in Smithay's typical `GlesRenderer` or render pipeline loops we can hook into to extract the raw framebuffer handle as an exportable allocation descriptor before it hits the local display output.
2. **Quinn Datagram Setup:** Provide a minimal Rust example using the `quinn` crate that initializes a server listener on a dedicated UDP port, turns on the `unreliable_datagrams` feature flag, and sets up a mock transmission pipeline.
3. **Reed-Solomon Integration Strategy:** Explain mathematically and structurally how we should chunk a 2MB video frame buffer into UDP-friendly sizes (~1200 bytes) while optimally injecting parity packets using the `reed-solomon-erasure` crate without creating severe CPU overhead on the encoder thread.
4. **Edge Cases & Deadlocks:** Flag any potential architectural deadlocks or multi-threading hazards between Smithay's main render loop thread and our async network socket worker loops.

Let's discuss the strategy first before writing files. Give me your conceptual breakdown.

