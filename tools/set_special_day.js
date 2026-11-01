const readline = require('readline');
const fs = require('fs');
const { join } = require('path');

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout
});

const ask = (query) => new Promise((resolve) => rl.question(query, resolve));

// Official WHS Alternate Bell Schedules from PDF
const BUILTIN_PRESETS = [
  {
    name: "Non-Pack Day",
    schedule: {
      lunch1: [
        { name: 'Period 1', start: '08:30', end: '09:59', duration: 89 },
        { name: 'Period 2', start: '10:07', end: '11:36', duration: 89 },
        { name: 'Lunch 1', start: '11:36', end: '12:06', duration: 30 },
        { name: 'Period 3', start: '12:14', end: '13:43', duration: 89 },
        { name: 'Period 4', start: '13:51', end: '15:20', duration: 89 }
      ],
      lunch2: [
        { name: 'Period 1', start: '08:30', end: '09:59', duration: 89 },
        { name: 'Period 2', start: '10:07', end: '11:36', duration: 89 },
        { name: 'Period 3', start: '11:44', end: '13:13', duration: 89 },
        { name: 'Lunch 2', start: '13:13', end: '13:43', duration: 30 },
        { name: 'Period 4', start: '13:51', end: '15:20', duration: 89 }
      ]
    }
  },
  {
    name: "Minimum Day",
    schedule: {
      lunch1: [
        { name: 'Period 1', start: '08:30', end: '09:26', duration: 56 },
        { name: 'Period 2', start: '09:34', end: '10:30', duration: 56 },
        { name: 'Period 3', start: '10:38', end: '11:34', duration: 56 },
        { name: 'Break / Lunch', start: '11:34', end: '11:46', duration: 12 },
        { name: 'Period 4', start: '11:54', end: '12:50', duration: 56 }
      ],
      lunch2: [
        { name: 'Period 1', start: '08:30', end: '09:26', duration: 56 },
        { name: 'Period 2', start: '09:34', end: '10:30', duration: 56 },
        { name: 'Period 3', start: '10:38', end: '11:34', duration: 56 },
        { name: 'Break / Lunch', start: '11:34', end: '11:46', duration: 12 },
        { name: 'Period 4', start: '11:54', end: '12:50', duration: 56 }
      ]
    }
  },
  {
    name: "Finals",
    schedule: {
      lunch1: [
        { name: 'Period 1/3', start: '08:30', end: '10:30', duration: 120 },
        { name: 'Break / Lunch', start: '10:30', end: '10:42', duration: 12 },
        { name: 'Period 2/4', start: '10:50', end: '12:50', duration: 120 }
      ],
      lunch2: [
        { name: 'Period 1/3', start: '08:30', end: '10:30', duration: 120 },
        { name: 'Break / Lunch', start: '10:30', end: '10:42', duration: 12 },
        { name: 'Period 2/4', start: '10:50', end: '12:50', duration: 120 }
      ]
    }
  }
];

const dataDir = join(__dirname, '..', 'data');
const presetsPath = join(dataDir, 'bell_presets.json');
const overridePath = join(dataDir, 'bell_overrides.json');

const SCHOOLS = {
  woodcreek: 'Woodcreek',
  roseville: 'Roseville',
  granitebay: 'Granite Bay',
  antelope: 'Antelope',
  westpark: 'West Park',
  oakmont: 'Oakmont'
};

// bell_overrides.json holds {"overrides": [{school, date, ...}]}. The older
// format was a single object for Woodcreek; it is converted on the next write.
function loadOverrides() {
  try {
    if (!fs.existsSync(overridePath)) return [];
    const doc = JSON.parse(fs.readFileSync(overridePath, 'utf8'));
    if (Array.isArray(doc.overrides)) return doc.overrides;
    if (doc && doc.date) return [{ school: 'woodcreek', ...doc }];
  } catch (e) {
    console.error('Error loading overrides:', e);
  }
  return [];
}

function loadPresets() {
  if (!fs.existsSync(dataDir)) {
    fs.mkdirSync(dataDir, { recursive: true });
  }
  let presets = [];
  try {
    if (fs.existsSync(presetsPath)) {
      presets = JSON.parse(fs.readFileSync(presetsPath, 'utf8'));
    }
  } catch (e) {
    console.error("Error loading presets:", e);
  }
  return presets;
}

function savePreset(name, schedule) {
  const presets = loadPresets();
  const idx = presets.findIndex(p => p.name.toLowerCase() === name.toLowerCase());
  if (idx !== -1) {
    presets[idx].schedule = schedule;
  } else {
    presets.push({ name, schedule });
  }
  fs.writeFileSync(presetsPath, JSON.stringify(presets, null, 2));
}

async function main() {
  console.log("\n=============================================");
  console.log("      SPECIAL BELL SCHEDULE OVERRIDE WIZARD  ");
  console.log("=============================================");

  const schoolKeys = Object.keys(SCHOOLS);
  console.log("\nWhich school is this for?");
  schoolKeys.forEach((key, idx) => console.log(`${idx + 1}) ${SCHOOLS[key]}`));
  const schoolChoice = parseInt(await ask(`Select school [1-${schoolKeys.length}]: `), 10) - 1;
  if (!(schoolChoice >= 0 && schoolChoice < schoolKeys.length)) {
    console.log("Invalid school. Nothing was changed.");
    rl.close();
    return;
  }
  const school = schoolKeys[schoolChoice];
  console.log(`\nSchool: ${SCHOOLS[school]}`);

  const customPresets = loadPresets();
  const allPresets = [...BUILTIN_PRESETS, ...customPresets];

  console.log("\nChoose schedule configuration option:");
  console.log("1) Use a Preset schedule (Built-in or Custom)");
  console.log("2) Create a new schedule from scratch");
  const modeChoice = await ask("Select option [1-2]: ");

  let chosenSchedule = null;
  let scheduleName = "Special Day";

  if (modeChoice.trim() === '1') {
    console.log("\n--- AVAILABLE PRESETS ---");
    allPresets.forEach((p, idx) => {
      const isBuiltin = idx < BUILTIN_PRESETS.length ? "Built-in" : "Custom";
      console.log(`${idx + 1}) ${p.name} (${isBuiltin})`);
    });
    const presetIdxStr = await ask(`Select preset [1-${allPresets.length}]: `);
    const presetIdx = parseInt(presetIdxStr) - 1;
    if (presetIdx >= 0 && presetIdx < allPresets.length) {
      const preset = allPresets[presetIdx];
      chosenSchedule = preset.schedule;
      scheduleName = preset.name;
      console.log(`\nSelected Preset: ${scheduleName}`);
    } else {
      console.log("Invalid selection. Defaulting to scratch creation.");
    }
  }

  if (!chosenSchedule) {
    console.log("\n--- CREATE NEW SCHEDULE FROM SCRATCH ---");
    scheduleName = await ask("\nEnter schedule name (e.g. Rally Day, Assembly Day): ") || "Special Day";
    
    const lunch1 = [];
    const lunch2 = [];
    
    let adding = true;
    while (adding) {
      const pName = await ask("\nPeriod Name (e.g. Period 1, Lunch 1, Rally): ");
      if (!pName || !pName.trim()) {
        console.log("Period name is required, please try again.");
        continue;
      }
      const start = await ask("Start Time (24h format HH:MM, e.g. 08:30): ");
      const end = await ask("End Time (24h format HH:MM, e.g. 09:40): ");
      
      // Parse duration
      let duration = 0;
      try {
        const [sh, sm] = start.split(':').map(Number);
        const [eh, em] = end.split(':').map(Number);
        duration = (eh * 60 + em) - (sh * 60 + sm);
      } catch (e) {}
      
      const lunchGroup = await ask("Which lunch does this period apply to? [1 = Lunch 1, 2 = Lunch 2, 3 = Both]: ");
      
      const periodObj = { name: pName.trim(), start: start.trim(), end: end.trim(), duration };
      
      const grp = String(lunchGroup).trim();
      if (grp === '1') {
        lunch1.push(periodObj);
      } else if (grp === '2') {
        lunch2.push(periodObj);
      } else {
        lunch1.push(periodObj);
        lunch2.push({ ...periodObj });
      }
      
      const more = await ask("\nAdd another period? (y/n): ");
      if (more.trim().toLowerCase() !== 'y') {
        adding = false;
      }
    }
    
    // Sort schedules by start time
    const sortByTime = (a, b) => {
      const [ah, am] = a.start.split(':').map(Number);
      const [bh, bm] = b.start.split(':').map(Number);
      return (ah * 60 + am) - (bh * 60 + bm);
    };
    lunch1.sort(sortByTime);
    lunch2.sort(sortByTime);

    chosenSchedule = { lunch1, lunch2 };

    const saveAsPreset = await ask("\nWould you like to save this new schedule as a preset for future use? (y/n): ");
    if (saveAsPreset.trim().toLowerCase() === 'y') {
      const presetName = await ask(`Enter preset name [Default: ${scheduleName}]: `) || scheduleName;
      savePreset(presetName, chosenSchedule);
      console.log(`✓ Preset "${presetName}" successfully saved to ${presetsPath}!`);
      scheduleName = presetName;
    }
  }

  // Choose date
  console.log("\nWhen is this special schedule for?");
  console.log("1) Today");
  console.log("2) Tomorrow");
  const dayChoice = await ask("Select option [1-2]: ");
  
  const now = new Date();
  const laTimeStr = now.toLocaleString("en-US", { timeZone: "America/Los_Angeles" });
  const laDate = new Date(laTimeStr);
  
  if (dayChoice === '2') {
    laDate.setDate(laDate.getDate() + 1);
  }
  
  const yyyy = laDate.getFullYear();
  const mm = String(laDate.getMonth() + 1).padStart(2, '0');
  const dd = String(laDate.getDate()).padStart(2, '0');
  const targetDateStr = `${yyyy}-${mm}-${dd}`;
  
  console.log(`\nSelected Target Date: ${targetDateStr}`);
  
  const specialEvent = await ask("\nEnter any special event notices (optional, e.g. Assembly in gym): ");
  
  const overrideObj = {
    school,
    date: targetDateStr,
    name: scheduleName,
    special_event: specialEvent ? specialEvent.trim() : null,
    schedule: chosenSchedule
  };
  
  // Keep other schools' overrides that haven't passed; replace this school's.
  const todayLA = new Date().toLocaleDateString('en-CA', { timeZone: 'America/Los_Angeles' });
  const kept = loadOverrides().filter(o => o.school !== school && o.date >= todayLA);
  fs.writeFileSync(overridePath, JSON.stringify({ overrides: [...kept, overrideObj] }, null, 2));

  console.log(`\n✓ Success! Special schedule successfully loaded into ${overridePath}`);
  console.log(`${SCHOOLS[school]} will show the "${scheduleName}" schedule on ${targetDateStr}!`);
  
  rl.close();
}

main();
