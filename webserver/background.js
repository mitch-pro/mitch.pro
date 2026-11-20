(() => {
  "use strict";
  if (window.MitchBackground || window.self !== window.top) return;
  const path = location.pathname;
  if (
    path.startsWith("/games/") &&
    !["/games/", "/games/index.html"].includes(path)
  )
    return;
  const root = document.documentElement;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");
  const choices = ["dark-clouds", "blue-clouds", "solid"];
  const number = (key, fallback, min, max) =>
    Math.max(
      min,
      Math.min(
        max,
        Number.isFinite(Number(read(key, String(fallback))))
          ? Number(read(key, String(fallback)))
          : fallback,
      ),
    );
  const hex = (key, fallback) =>
    /^#[a-f0-9]{6}$/i.test(read(key, "")) ? read(key, "") : fallback;
  const colors = {
    "dark-clouds": {
      backgroundColor: 0x000000,
      skyColor: 0x030507,
      cloudColor: 0x283039,
      cloudShadowColor: 0x050608,
      sunColor: 0x454b50,
      sunGlareColor: 0x15191d,
      sunlightColor: 0x69727a,
    },
    "blue-clouds": {
      backgroundColor: 0xffffff,
      skyColor: 0x68b8d7,
      cloudColor: 0xadc1de,
      cloudShadowColor: 0x183550,
      sunColor: 0xff9919,
      sunGlareColor: 0xff6633,
      sunlightColor: 0xff9933,
    },
  };
  let layer,
    effect,
    effectKey = "",
    loading,
    dialog,
    stopped = false,
    unavailable = false,
    generation = 0,
    idle;
  const read = (key, fallback) => {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch (_) {
      return fallback;
    }
  };
  const write = (key, value) => {
    try {
      localStorage.setItem(key, value);
    } catch (_) {}
  };
  const selection = () => {
    const value = read("theme_background", "dark-clouds");
    return choices.includes(value) ? value : "dark-clouds";
  };
  function hasCustomBackground() {
    const profileImage = document.body?.style.backgroundImage;
    return (
      !!read("theme_bgimg", "") ||
      root.hasAttribute("data-bg-effect") ||
      (!!profileImage && profileImage !== "none")
    );
  }
  function motionAllowed() {
    return (
      !reduced.matches &&
      read("theme_motion", "on") !== "off" &&
      read("theme_clouds_motion", "on") !== "off" &&
      !root.classList.contains("theme-no-motion")
    );
  }
  function stopEffect() {
    if (idle) {
      if ("cancelIdleCallback" in window) cancelIdleCallback(idle);
      else clearTimeout(idle);
      idle = null;
    }
    if (effect) {
      const instance = effect,
        renderer = instance.renderer;
      effect = null;
      effectKey = "";
      instance.destroy();
      renderer?.dispose();
      renderer?.forceContextLoss();
    }
    if (layer) layer.dataset.render = "static";
  }
  function loadScript(source) {
    return new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = source;
      script.async = true;
      script.onload = () => {
        script.remove();
        resolve();
      };
      script.onerror = () => {
        script.remove();
        reject(new Error("Background unavailable"));
      };
      document.head.append(script);
    });
  }
  function dependencies() {
    if (!loading)
      loading = (async () => {
        if (!window.THREE) await loadScript("/vendor/three.r134.min.js");
        if (!window.VANTA?.CLOUDS)
          await loadScript("/vendor/vanta.clouds.min.js");
      })().catch((error) => {
        loading = null;
        throw error;
      });
    return loading;
  }
  async function startEffect(preset, version) {
    idle = null;
    try {
      await dependencies();
      if (
        stopped ||
        version !== generation ||
        document.hidden ||
        !motionAllowed() ||
        hasCustomBackground()
      )
        return;
      effect = window.VANTA.CLOUDS({
        el: layer,
        THREE: window.THREE,
        ...colors[preset],
        minWidth: 160,
        minHeight: 160,
        mouseControls: read("theme_clouds_pointer", "on") !== "off",
        touchControls: false,
        gyroControls: false,
        speed: number("theme_clouds_speed", 0.5, 0.1, 1.5),
        scale: Math.max(3, (window.devicePixelRatio || 1) * 2),
        scaleMobile: Math.max(4, (window.devicePixelRatio || 1) * 3),
      });
      if (!effect.renderer) throw new Error("WebGL unavailable");
      effectKey =
        preset +
        ":" +
        read("theme_clouds_speed", ".5") +
        ":" +
        read("theme_clouds_pointer", "on");
      layer.dataset.render = "animated";
      const canvas = effect.renderer.domElement;
      canvas.setAttribute("aria-hidden", "true");
      canvas.addEventListener(
        "webglcontextlost",
        () => {
          if (effect?.renderer?.domElement === canvas) {
            unavailable = true;
            stopEffect();
          }
        },
        { once: true },
      );
    } catch (_) {
      unavailable = true;
      stopEffect();
    }
  }
  function sync() {
    if (!layer) return;
    const preset = selection(),
      custom = hasCustomBackground();
    layer.dataset.background = preset;
    layer.style.backgroundColor =
      preset === "solid" ? hex("theme_solid_color", "var(--t-bg)") : "";
    layer.style.setProperty(
      "--site-bg-dim",
      String(
        number(
          "theme_background_dim",
          preset === "blue-clouds" ? 0.6 : 0.13,
          0,
          0.85,
        ),
      ),
    );
    const key =
      preset +
      ":" +
      read("theme_clouds_speed", ".5") +
      ":" +
      read("theme_clouds_pointer", "on");
    layer.hidden = custom;
    document.body.classList.toggle("site-background-enabled", !custom);
    const constrained =
      navigator.connection?.saveData ||
      (navigator.deviceMemory && navigator.deviceMemory <= 2);
    const animate =
      !custom &&
      preset !== "solid" &&
      motionAllowed() &&
      !constrained &&
      !unavailable &&
      !stopped &&
      !path.startsWith("/matrix/") &&
      !!window.WebGLRenderingContext;
    if (!animate) {
      generation++;
      stopEffect();
      updateDialog();
      return;
    }
    if (effect && effectKey === key) {
      if (document.hidden) cancelAnimationFrame(effect.req);
      else if (layer.dataset.render === "paused") {
        effect.animationLoop();
        layer.dataset.render = "animated";
      }
      if (document.hidden) layer.dataset.render = "paused";
      updateDialog();
      return;
    }
    if (document.hidden) return;
    generation++;
    stopEffect();
    const version = generation;
    idle =
      "requestIdleCallback" in window
        ? requestIdleCallback(() => startEffect(preset, version), {
            timeout: 1200,
          })
        : setTimeout(() => startEffect(preset, version), 100);
    updateDialog();
  }
  function updateDialog() {
    if (!dialog) return;
    dialog.querySelectorAll('[name="site-background"]').forEach((input) => {
      input.checked = input.value === selection();
    });
    const toggle = dialog.querySelector("#clouds-animation");
    toggle.checked = read("theme_clouds_motion", "on") !== "off";
    toggle.disabled =
      reduced.matches ||
      read("theme_motion", "on") === "off" ||
      selection() === "solid";
    dialog.querySelector("#clouds-speed").value = number(
      "theme_clouds_speed",
      0.5,
      0.1,
      1.5,
    );
    dialog.querySelector("#clouds-speed-value").value =
      number("theme_clouds_speed", 0.5, 0.1, 1.5).toFixed(1) + "×";
    dialog.querySelector("#background-dim").value = Math.round(
      number(
        "theme_background_dim",
        selection() === "blue-clouds" ? 0.6 : 0.13,
        0,
        0.85,
      ) * 100,
    );
    dialog.querySelector("#background-dim-value").value =
      dialog.querySelector("#background-dim").value + "%";
    dialog.querySelector("#clouds-pointer").checked =
      read("theme_clouds_pointer", "on") !== "off";
    dialog.querySelector("#solid-color").value = hex(
      "theme_solid_color",
      "#000000",
    );
    dialog.querySelector("#customize-accent").value = /^#[a-f0-9]{6}$/i.test(
      window.__siteTheme?.getAccent?.() || "",
    )
      ? window.__siteTheme.getAccent()
      : getComputedStyle(root).getPropertyValue("--t-ac").trim() || "#9acbfa";
    dialog.querySelector("#customize-font").value = read(
      "theme_font",
      "arcade",
    );
    dialog.querySelector("#customize-density").value = read(
      "theme_density",
      "normal",
    );
    dialog.querySelector("#customize-contrast").checked =
      window.__siteTheme?.getContrast?.() === "high";
    dialog.querySelector("#solid-color-field").hidden = selection() !== "solid";
    dialog.querySelector("#clouds-speed").disabled =
      toggle.disabled || !toggle.checked;
    dialog.querySelector("#clouds-pointer").disabled =
      toggle.disabled || !toggle.checked;
    dialog.querySelector("[data-background-note]").textContent = reduced.matches
      ? "Reduced motion is enabled on your device."
      : read("theme_motion", "on") === "off"
        ? "Motion is off in your appearance settings."
        : "";
  }
  function customize(opener) {
    if (!dialog) {
      dialog = document.createElement("dialog");
      dialog.className = "background-dialog";
      dialog.id = "background-dialog";
      dialog.setAttribute("aria-labelledby", "background-heading");
      dialog.innerHTML =
        '<header><h2 id="background-heading">Customize background</h2><button type="button" data-background-close aria-label="Close background settings">×</button></header><fieldset><legend>Background</legend><div class="background-choices"><label class="background-choice"><span class="background-preview" aria-hidden="true"></span><span>Dark clouds</span><input type="radio" name="site-background" value="dark-clouds"></label><label class="background-choice"><span class="background-preview blue" aria-hidden="true"></span><span>Blue clouds</span><input type="radio" name="site-background" value="blue-clouds"></label><label class="background-choice"><span class="background-preview solid" aria-hidden="true"></span><span>Solid background</span><input type="radio" name="site-background" value="solid"></label></div></fieldset><label class="background-motion"><input type="checkbox" id="clouds-animation">Animate clouds</label><p class="background-note" data-background-note role="status"></p>';
      dialog.querySelector("h2").textContent = "Customize";
      dialog.insertAdjacentHTML(
        "beforeend",
        `<section class="background-options"><h3>Background settings</h3>
        <label id="solid-color-field">Background color<input id="solid-color" type="color"></label>
        <label>Dim background<output id="background-dim-value" for="background-dim"></output><input id="background-dim" type="range" min="0" max="85" step="1"></label>
        <label>Cloud speed<output id="clouds-speed-value" for="clouds-speed"></output><input id="clouds-speed" type="range" min="0.1" max="1.5" step="0.1"></label>
        <label class="background-check"><input id="clouds-pointer" type="checkbox">Respond to pointer movement</label>
        </section><section class="background-options"><h3>Appearance</h3>
        <label>Accent color<input id="customize-accent" type="color"></label>
        <label>Text style<select id="customize-font"><option value="arcade">Arcade</option><option value="system">System sans</option><option value="plain">Plain</option><option value="mono">Monospace</option><option value="rounded">Rounded</option><option value="serif">Serif</option><option value="futuristic">Trebuchet</option></select></label>
        <label>Text size<select id="customize-density"><option value="compact">Small</option><option value="normal">Default</option><option value="comfy">Large</option><option value="huge">Largest</option></select></label>
        <label class="background-check"><input id="customize-contrast" type="checkbox">High contrast</label>
        </section><footer><a href="/preferences/#appearance">All appearance settings</a><button type="button" data-customize-reset>Reset customization</button></footer>`,
      );
      dialog.addEventListener("input", (event) => {
        if (event.target.id === "clouds-speed")
          dialog.querySelector("#clouds-speed-value").value =
            Number(event.target.value).toFixed(1) + "×";
        if (event.target.id === "background-dim")
          dialog.querySelector("#background-dim-value").value =
            event.target.value + "%";
      });
      dialog.querySelector("[data-customize-reset]").onclick = () => {
        for (const key of [
          "theme_background",
          "theme_clouds_motion",
          "theme_clouds_speed",
          "theme_clouds_pointer",
          "theme_background_dim",
          "theme_solid_color",
          "theme_font",
          "theme_density",
        ]) {
          try {
            localStorage.removeItem(key);
          } catch (_) {}
        }
        window.__siteTheme?.setAccent("");
        window.__siteTheme?.setContrast("");
        window.__siteTheme?.setMode(window.__siteTheme.getMode());
        sync();
      };
      document.body.append(dialog);
      dialog
        .querySelector("[data-background-close]")
        .addEventListener("click", () => dialog.close());
      dialog.addEventListener("click", (event) => {
        if (event.target === dialog) {
          const box = dialog.getBoundingClientRect();
          if (
            event.clientX < box.left ||
            event.clientX > box.right ||
            event.clientY < box.top ||
            event.clientY > box.bottom
          )
            dialog.close();
        }
      });
      dialog.addEventListener("change", (event) => {
        if (event.target.name === "site-background") {
          write("theme_background", event.target.value);
          // A deliberate preset selection replaces the user's previous wallpaper.
          write("theme_bgimg", "");
          document.cookie = "bgimg=; path=/; max-age=0; SameSite=Lax";
          window.__theme?.setBg("");
        } else {
          const target = event.target;
          const fields = {
            "clouds-speed": "theme_clouds_speed",
            "background-dim": "theme_background_dim",
            "solid-color": "theme_solid_color",
            "customize-font": "theme_font",
            "customize-density": "theme_density",
          };
          if (fields[target.id])
            write(
              fields[target.id],
              target.id === "background-dim"
                ? String(Number(target.value) / 100)
                : target.value,
            );
          if (target.id === "clouds-animation")
            write("theme_clouds_motion", target.checked ? "on" : "off");
          if (target.id === "clouds-pointer")
            write("theme_clouds_pointer", target.checked ? "on" : "off");
          if (target.id === "customize-accent")
            window.__siteTheme?.setAccent(target.value);
          if (target.id === "customize-contrast")
            window.__siteTheme?.setContrast(target.checked ? "high" : "");
          if (["customize-font", "customize-density"].includes(target.id))
            window.__siteTheme?.setMode(window.__siteTheme.getMode());
        }
        sync();
      });
    }
    updateDialog();
    if (dialog.open) return;
    dialog.addEventListener(
      "close",
      () => {
        if (opener?.isConnected) opener.focus({ preventScroll: true });
      },
      { once: true },
    );
    dialog.showModal();
  }
  function initialize() {
    layer = document.createElement("div");
    layer.id = "site-background";
    layer.setAttribute("aria-hidden", "true");
    document.body.prepend(layer);
    document.addEventListener("click", (event) => {
      const opener =
        event.target instanceof Element &&
        event.target.closest("[data-background-customize]");
      if (opener) customize(opener);
    });
    new MutationObserver(sync).observe(document.body, {
      attributes: true,
      attributeFilter: ["style"],
    });
    new MutationObserver(sync).observe(root, {
      attributes: true,
      attributeFilter: ["class", "data-theme", "data-bg-effect"],
    });
    sync();
  }
  window.MitchBackground = { customize, refresh: sync };
  reduced.addEventListener("change", sync);
  window.addEventListener("storage", (event) => {
    if (!event.key || event.key.startsWith("theme_")) sync();
  });
  window.addEventListener("themechange", sync);
  document.addEventListener("visibilitychange", sync);
  window.addEventListener("pagehide", () => {
    stopped = true;
    generation++;
    stopEffect();
  });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) {
      stopped = false;
      sync();
    }
  });
  if (document.readyState === "loading")
    document.addEventListener("DOMContentLoaded", initialize, { once: true });
  else initialize();
})();
