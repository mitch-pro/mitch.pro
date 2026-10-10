(()=>{"use strict";
const $=id=>document.getElementById(id);

const SCHOOLS = {
 woodcreek: {name:"Woodcreek",calendar:"calendar",bell:"about/bell-schedules",lat:38.771,lon:-121.367},
 roseville: {name:"Roseville",calendar:"school-calendar",bell:"about/bell-schedules",lat:38.752,lon:-121.285},
 granitebay: {name:"Granite Bay",calendar:"calendar",bell:"about/bell-schedules",lat:38.742,lon:-121.222},
 antelope: {name:"Antelope",calendar:"antelope-hs-calendar",bell:"about/bell-schedule",lat:38.725,lon:-121.366},
 westpark: {name:"West Park",calendar:"panther-calendar",bell:"about/bell-schedules",lat:38.792,lon:-121.394},
 oakmont: {name:"Oakmont",calendar:"calendar",bell:"about/bell-schedules",lat:38.724,lon:-121.270}
};
function saved(key, fallback=null){try{return localStorage.getItem(key)||fallback}catch{return fallback}}
function remember(key,value){try{localStorage.setItem(key,value)}catch{}}
// Clean path (/woodcreek/) is the real, canonical, indexable URL now —
// ?school= is only a legacy/compat input (old links get 301'd server-side,
// but this still reads it client-side just in case something bypasses that).
let school = location.pathname.replace(/^\/|\/$/g,"") || new URLSearchParams(location.search).get("school") || saved("rjuhsd_school","woodcreek");
if(!SCHOOLS[school]) school="woodcreek";
let events=[],requestVersion=0,scheduleMode="auto",includePeriod0=false,calendarVerified=false,bellOverride=null;
function schoolName(){return SCHOOLS[school].name+" High School"}
function official(){return "https://"+school+".rjuhsd.us"}
function dayData(date){return window.RJUHSD_CALENDAR.resolve(school,date,events,date===pacific().date?scheduleMode:"auto",includePeriod0,bellOverride)}
function buildData(){
 const date=pacific().date,weekdayName=new Date(date+"T12:00:00Z").toLocaleDateString("en-US",{weekday:"long",timeZone:"UTC"});
 const today=dayData(date),base=new Date(date+"T12:00:00Z");base.setUTCDate(base.getUTCDate()-((base.getUTCDay()+6)%7));
 const week=Array.from({length:5},(_,i)=>{const d=new Date(base);d.setUTCDate(d.getUTCDate()+i);const key=d.toISOString().slice(0,10),v=dayData(key),s=lunch==="2"?v.lunch2:v.lunch1;return {...v,weekdayName:d.toLocaleDateString("en-US",{weekday:"long",timeZone:"UTC"}),isToday:key===date,short:v.title,firstBell:s[0]?.start,lastBell:s.at(-1)?.end}});
 return {now:{iso:date,weekdayName},today,week,officialEvents:events,upcoming:[],menu:{}};
}
function applySchoolIdentity(){
 document.body.dataset.school=school;
 const brand=window.RJUHSD_SCHOOL_DATA[school];
 document.documentElement.style.setProperty("--school-primary",brand.primary);
 document.documentElement.style.setProperty("--school-secondary",brand.secondary);
 document.querySelectorAll('img[src*="/rjuhsd-assets/"]:not(.brand-logo img):not(.site-logo):not(.district-school-logo)').forEach(img=>{img.src=brand.logo;img.alt=img.closest('[aria-hidden="true"]')?"":schoolName()+" logo"});
 const fi=document.querySelector('link[rel="icon"]');if(fi)fi.href="/favicon.ico";
 const tc=document.querySelector('meta[name="theme-color"]');if(tc)tc.content=document.documentElement.classList.contains("theme-light")?"#f7f4f4":"#0c0809";
 $("schedule-mode").innerHTML='<option value="auto">Automatic</option><option value="regular">Regular day</option>'+Object.keys(brand.specials).map(k=>'<option value="'+k+'">'+safe(window.RJUHSD_CALENDAR.labels[k])+'</option>').join("");
 $("schedule-mode").value=scheduleMode;
 $("period0-label").hidden=!brand.days[1][1].some(p=>p.name==="Period 0");
 document.title=schoolName()+" Bell Schedule | RJUHSD Hub";
 $("school-heading").textContent=SCHOOLS[school].name;
 $("hero-overline").textContent=schoolName();
 document.querySelectorAll(".brand-copy strong").forEach(e=>e.textContent=schoolName());
 document.querySelectorAll(".brand-copy small").forEach(e=>e.textContent="rjuhsd.school");
 document.querySelectorAll(".district-school-card").forEach(c=>{c.classList.toggle("current-school",c.dataset.schoolCard===school)});
 $("official-bells").href=official()+"/"+SCHOOLS[school].bell;
 document.querySelectorAll('a[href*="woodcreek.rjuhsd.us"]:not(#official-bells)').forEach(a=>a.dataset.schoolLink="true");
 document.querySelectorAll("[data-school-link]").forEach(a=>{a.href=official()+"/"+SCHOOLS[school].calendar;if(a.querySelector(".tool-logo")){a.href=official();a.querySelector("strong").textContent="School website"}});
 document.querySelectorAll('a[href*="maps/search"]').forEach(a=>{a.href="https://www.google.com/maps/search/?api=1&query="+encodeURIComponent(schoolName());a.querySelector("small").textContent="Campus directions"});
 document.querySelector(".weather-place strong").textContent=SCHOOLS[school].name;
}

let lunch=saved("rjuhsd_lunch_"+school,"1"),data=buildData(),calendarCursor;
function pacific(){const p=Object.fromEntries(new Intl.DateTimeFormat("en-US",{timeZone:"America/Los_Angeles",year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false}).formatToParts(new Date()).map(x=>[x.type,x.value]));return{date:`${p.year}-${p.month}-${p.day}`,minutes:(+p.hour%24)*60+(+p.minute),seconds:+p.second}}
function mins(v){if(!v)return 0;const[h,m]=v.split(":").map(Number);return h*60+m}
function time(v){if(!v)return"—";let[h,m]=v.split(":").map(Number),s=h>=12?"PM":"AM";h=h%12||12;return`${h}:${String(m).padStart(2,"0")} ${s}`}
function schedule(){if(data.today?.oneLunch)return data.today?.lunch1||data.today?.lunch2||[];return(lunch==="2"?data.today?.lunch2:data.today?.lunch1)||[]}
function duration(a,b){const n=Math.max(0,mins(b)-mins(a));return`${Math.floor(n/60)}h ${n%60}m`}
function toast(msg){const e=$("toast");e.textContent=msg;e.classList.add("show");clearTimeout(toast.t);toast.t=setTimeout(()=>e.classList.remove("show"),2200)}
function openLunch(){const combined=!!data.today?.oneLunch;$("onboarding-title").textContent=combined?"Today uses one lunch":"Which lunch are you?";$("onboarding-description").textContent=combined?"Today automatically follows the combined lunch schedule. Choose your usual lunch for other school days.":"Choose once and every bell, countdown, and timeline will match your school day. You can change this anytime.";const m=$("onboarding");m.hidden=false;document.body.style.overflow="hidden";setTimeout(()=>m.querySelector("button")?.focus(),40)}
function chooseLunch(value){lunch=value;remember("rjuhsd_lunch_"+school,value);remember("rjuhsd_onboarded_"+school,"yes");$("onboarding").hidden=true;document.body.style.overflow="";data=buildData();render();toast(`Lunch ${value} schedule is now active`)}

function renderIdentity(){const iso=data.now?.iso||pacific().date,d=new Date(`${iso}T12:00:00`),combined=!!data.today?.oneLunch;$("hero-date").dateTime=iso;$("hero-weekday").textContent=data.now?.weekdayName||d.toLocaleDateString("en-US",{weekday:"long"});$("hero-date-day").textContent=String(d.getDate()).padStart(2,"0");$("hero-date-month").textContent=d.toLocaleDateString("en-US",{month:"long"});$("hero-date-year").textContent=d.getFullYear();$("date-month").textContent=d.toLocaleDateString("en-US",{month:"short"}).toUpperCase();$("date-day").textContent=String(d.getDate()).padStart(2,"0");$("date-year").textContent=d.getFullYear();$("hero-summary").textContent=data.today?.blurb||data.today?.event||"Regular weekly schedule.";$("header-lunch").textContent=combined?"Combined lunch":lunch?`Lunch ${lunch}`:"Choose lunch";$("brief-lunch").textContent=combined?"COMBINED LUNCH":`LUNCH ${lunch||"—"}`;$("schedule-note").textContent=combined?"Today uses a combined lunch.":`Showing times for Lunch ${lunch||"—"}.`;$("snapshot-type").textContent=(data.today?.title||"Today").replace(" schedule","");}
function renderTimeline(){const s=schedule(),now=pacific();if(!data.today?.inSession||!s.length){$("timeline").innerHTML=`<div class="timeline-row active"><span class="time">TODAY</span><i class="timeline-dot"></i><span class="period-name">${safe(data.today?.event||"No school")}</span><span class="period-meta"></span></div>`;["snapshot-first","snapshot-last","day-length","period-count","pack-placement"].forEach((id,i)=>$(id).textContent=i?"—":data.today?.unavailable?"Unverified":"No school");return}$("timeline").innerHTML=s.map(x=>{const active=now.date===data.now?.iso&&now.minutes>=mins(x.start)&&now.minutes<mins(x.end),passed=now.date===data.now?.iso&&now.minutes>=mins(x.end);return`<div class="timeline-row${active?" active":""}${passed?" passed":""}"><span class="time">${time(x.start).replace(" ","<br>")}</span><i class="timeline-dot"></i><span class="period-name">${safe(x.name)}${active?'<b class="now-chip">NOW</b>':""}</span><span class="period-meta">${mins(x.end)-mins(x.start)} min<br>${time(x.end)}</span></div>`}).join("");$("snapshot-first").textContent=time(s[0].start);$("snapshot-last").textContent=time(s.at(-1).end);$("day-length").textContent=duration(s[0].start,s.at(-1).end);$("period-count").textContent=`${s.filter(x=>/^Period [1-4]$/.test(x.name)).length} blocks`;const pack=s.find(x=>/Pack|Roar|Titan|Panther|Intervention/i.test(x.name)),idx=s.indexOf(pack),prev=s.slice(0,idx).reverse().find(x=>/Period/.test(x.name));$("pack-placement").textContent=pack?(prev?`After ${prev.name}`:time(pack.start)):"None today"}
function renderWeek(){const s=schedule(),fallback=[{weekdayName:data.now?.weekdayName||"Today",date:data.now?.iso,short:data.today?.title,isToday:true,inSession:data.today?.inSession,firstBell:s[0]?.start,lastBell:s.at(-1)?.end}],week=data.week?.length?data.week:fallback;$("week-grid").innerHTML=week.map(x=>{const d=x.date?new Date(`${x.date}T12:00:00`):null;return`<article class="day-card${x.isToday?" today":""}${!x.inSession?" off":""}"><div class="day-top"><span>${x.weekdayName}</span>${x.isToday?'<b class="today-pill">TODAY</b>':""}</div><span class="day-date">${d?d.toLocaleDateString("en-US",{month:"short",day:"numeric"}):""}</span><p>${safe(x.event||x.short||"Regular schedule")}</p><strong>${x.inSession?`${time(x.firstBell)} – ${time(x.lastBell)}`:x.unavailable?"Unverified":"No school"}</strong></article>`}).join("")}
function safe(value){return String(value??"").replace(/[&<>"']/g,x=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[x])}
function eventData(){const official=(data.officialEvents||[]).map(x=>({...x,source:schoolName()})),officialDates=new Set(official.map(x=>x.date)),scheduleEvents=(data.upcoming||[]).filter(x=>!officialDates.has(x.date)).map(x=>({date:x.date,text:x.text,source:"Schedule"})),seen=new Set;return[...official,...scheduleEvents].filter(x=>x.date&&x.text&&!seen.has(`${x.date}|${x.text.toLowerCase()}`)&&seen.add(`${x.date}|${x.text.toLowerCase()}`)).sort((a,b)=>a.date.localeCompare(b.date))}
function renderEvents(){const events=eventData(),today=data.now?.iso||pacific().date,todayDate=new Date(`${today}T12:00:00`);if(!calendarCursor)calendarCursor={year:todayDate.getFullYear(),month:todayDate.getMonth()};const monthStart=new Date(Date.UTC(calendarCursor.year,calendarCursor.month,1)),first=monthStart.getUTCDay(),days=new Date(Date.UTC(calendarCursor.year,calendarCursor.month+1,0)).getUTCDate(),byDate=new Map;events.forEach(x=>{const list=byDate.get(x.date)||[];list.push(x);byDate.set(x.date,list)});$("calendar-title").textContent=monthStart.toLocaleDateString("en-US",{timeZone:"UTC",month:"long",year:"numeric"});const cells=[];for(let i=0;i<42;i++){const day=i-first+1;if(day<1||day>days){cells.push('<span class="outside" aria-hidden="true"></span>');continue}const key=`${calendarCursor.year}-${String(calendarCursor.month+1).padStart(2,"0")}-${String(day).padStart(2,"0")}`,items=byDate.get(key)||[],labels=items.map(x=>x.text).join(", "),classes=[key===today?"today":"",items.length?"has-event":""].filter(Boolean).join(" ");cells.push(`<button type="button" class="${classes}"${key===today?' aria-current="date"':""} aria-label="${safe(new Date(`${key}T12:00:00`).toLocaleDateString("en-US",{month:"long",day:"numeric",year:"numeric"}))}${labels?`: ${safe(labels)}`:""}" title="${safe(labels)}"><span>${day}</span>${items.length?`<i>${items.length}</i>`:""}</button>`)}$("calendar-days").innerHTML=cells.join("");const upcoming=events.filter(x=>x.date>=today).slice(0,6),visible=upcoming.length?upcoming:events.slice(-6);$("event-list").innerHTML=visible.length?visible.map(x=>{const d=new Date(`${x.date}T12:00:00`);return`<li><time datetime="${x.date}"><span>${d.toLocaleDateString("en-US",{month:"short"}).toUpperCase()}</span><strong>${String(d.getDate()).padStart(2,"0")}</strong></time><span><small>${d.toLocaleDateString("en-US",{weekday:"long"}).toUpperCase()} · ${safe(x.source||"SCHOOL CALENDAR")}</small><strong>${safe(x.text)}</strong></span></li>`}).join(""):'<li class="agenda-empty"><strong>No new events posted.</strong><span>Check the official calendar for updates.</span></li>';const next=upcoming[0];$("announcement-text").textContent=next?`Next up: ${next.text} · ${new Date(`${next.date}T12:00:00`).toLocaleDateString("en-US",{weekday:"short",month:"short",day:"numeric"})}`:(data.today?.blurb||"Regular weekly schedule.")}
function moveCalendar(offset){const d=new Date(calendarCursor.year,calendarCursor.month+offset,1);calendarCursor={year:d.getFullYear(),month:d.getMonth()};renderEvents()}
function weatherInfo(code){if(code===0)return["☀","Clear"];if(code<=2)return["⛅","Partly cloudy"];if(code===3)return["☁","Cloudy"];if(code<=48)return["≋","Foggy"];if(code<=67||code>=80&&code<=82)return["🌧","Rain"];if(code<=77)return["❄","Snow"];if(code>=95)return["⛈","Thunderstorms"];return["☀","Fair"]}
function renderWeather(w){const current=w.current||{},daily=w.daily||{},info=weatherInfo(Number(current.weather_code));$("weather-symbol").textContent=info[0];$("weather-condition").textContent=info[1];$("weather-temperature").textContent=`${Math.round(current.temperature_2m)}°`;$("weather-feels").textContent=`${Math.round(current.apparent_temperature)}°`;$("weather-wind").textContent=`${Math.round(current.wind_speed_10m)} mph`;$("weather-range").textContent=`${Math.round(daily.temperature_2m_max?.[0])}° / ${Math.round(daily.temperature_2m_min?.[0])}°`;$("weather-forecast").innerHTML=(daily.time||[]).slice(0,7).map((date,i)=>{const d=new Date(`${date}T12:00:00`),day=i===0?"Today":d.toLocaleDateString("en-US",{weekday:"short"}),dayInfo=weatherInfo(Number(daily.weather_code?.[i]));return`<div><span>${day}</span><i aria-hidden="true">${dayInfo[0]}</i><strong>${Math.round(daily.temperature_2m_max?.[i])}°</strong><small>${Math.round(daily.temperature_2m_min?.[i])}°</small></div>`}).join("");$("weather-updated").textContent=`Updated ${new Intl.DateTimeFormat("en-US",{hour:"numeric",minute:"2-digit"}).format(new Date())} · ${SCHOOLS[school].name}, CA`}
async function loadWeather(){
 const selected=school,s=SCHOOLS[selected];
 try{
 const r=await fetch("https://api.open-meteo.com/v1/forecast?latitude="+s.lat+"&longitude="+s.lon+"&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&temperature_unit=fahrenheit&wind_speed_unit=mph&timezone=America%2FLos_Angeles&forecast_days=7",{signal:AbortSignal.timeout(10000)});
 if(!r.ok){r.text().catch(()=>{});throw Error();}const w=await r.json();if(selected!==school)return;if(!Number.isFinite(w.current?.temperature_2m))throw Error();renderWeather(w);
 }catch{if(selected===school){$("weather-condition").textContent="Forecast unavailable";$("weather-updated").textContent="Weather service is temporarily unavailable"}}
}
function renderNow(){const p=pacific(),s=schedule(),clock=new Intl.DateTimeFormat("en-US",{timeZone:"America/Los_Angeles",hour:"numeric",minute:"2-digit"}).format(new Date());$("live-clock").textContent=clock;$("header-clock").textContent=clock;if(!data.today?.inSession||data.today?.manual||p.date!==data.now?.iso||!s.length){$("live-mode").textContent=data.today?.manual?"PREVIEW":p.date===data.now?.iso?"TODAY":"PREVIEW";$("current-eyebrow").textContent="TODAY AT "+SCHOOLS[school].name.toUpperCase();$("current-period").textContent=data.today?.event||data.today?.title||"Schedule preview";$("current-range").textContent=schoolName();$("countdown").textContent="—";$("countdown-copy").textContent="No live bell countdown right now";$("live-progress").style.width="0";$("next-period").textContent=data.nextSchool?.title||"Check back soon";$("next-time").textContent=data.nextSchool?.date||"—";$("snapshot-progress").textContent="Not in session";return}const now=p.minutes+p.seconds/60,current=s.find(x=>now>=mins(x.start)&&now<mins(x.end)),next=s.find(x=>mins(x.start)>now),dayPct=Math.min(100,Math.max(0,(now-mins(s[0].start))/(mins(s.at(-1).end)-mins(s[0].start))*100));$("snapshot-progress").textContent=`${Math.round(dayPct)}% complete`;if(current){const sec=Math.max(0,Math.round((mins(current.end)-now)*60));$("live-mode").textContent="LIVE";$("current-eyebrow").textContent="HAPPENING NOW";$("current-period").textContent=current.name;$("current-range").textContent=`${time(current.start)} – ${time(current.end)}`;$("countdown").textContent=`${String(Math.floor(sec/60)).padStart(2,"0")}:${String(sec%60).padStart(2,"0")}`;$("countdown-copy").textContent="until the next bell";$("period-badge").textContent=current.name.match(/\d/)?.[0]||current.name[0];$("live-progress").style.width=`${(now-mins(current.start))/(mins(current.end)-mins(current.start))*100}%`;const n=s[s.indexOf(current)+1];$("next-period").textContent=n?.name||"Dismissal";$("next-time").textContent=n?time(n.start):time(current.end)}else if(next){const sec=Math.round((mins(next.start)-now)*60);$("live-mode").textContent=now<mins(s[0].start)?"STARTING SOON":"PASSING";$("current-eyebrow").textContent=now<mins(s[0].start)?"BEFORE SCHOOL":"PASSING PERIOD";$("current-period").textContent=now<mins(s[0].start)?"School starts soon":"Head to class";$("current-range").textContent=`Next bell at ${time(next.start)}`;$("countdown").textContent=`${String(Math.floor(sec/60)).padStart(2,"0")}:${String(sec%60).padStart(2,"0")}`;$("countdown-copy").textContent="until class begins";$("period-badge").textContent="→";$("live-progress").style.width="0";$("next-period").textContent=next.name;$("next-time").textContent=time(next.start)}else{$("live-mode").textContent="DONE";$("current-eyebrow").textContent="SCHOOL’S OUT";$("current-period").textContent="That’s a wrap";$("current-range").textContent=`Dismissed at ${time(s.at(-1).end)}`;$("countdown").textContent="DONE";$("countdown-copy").textContent="See you next school day";$("period-badge").textContent="✓";$("live-progress").style.width="100%";$("next-period").textContent="Next school day";$("next-time").textContent="—"}}
function setDial(percent,end,context,label="ENDS AT"){const value=Math.min(100,Math.max(0,percent||0)),ring=$("countdown-ring");ring.style.strokeDashoffset=String(326.73*(1-value/100));$("countdown-percent").textContent=`${Math.round(value)}%`;$("countdown-end").previousElementSibling.textContent=label;$("countdown-end").textContent=end||"—";$("countdown-context").textContent=context||"—"}
function renderNowAdvanced(){
  const p=pacific(),s=schedule(),clock=new Intl.DateTimeFormat("en-US",{timeZone:"America/Los_Angeles",hour:"numeric",minute:"2-digit"}).format(new Date());
  $("live-clock").textContent=clock;$("header-clock").textContent=clock;
  if(p.date!==data.now?.iso&&!load.pending){load.pending=true;load().finally(()=>{load.pending=false})}
  if(!data.today?.inSession||p.date!==data.now?.iso||!s.length){$("live-mode").textContent=p.date===data.now?.iso?"TODAY":"PREVIEW";$("current-eyebrow").textContent="TODAY AT "+SCHOOLS[school].name.toUpperCase();$("current-period").textContent=data.today?.event||data.today?.title||"Schedule preview";$("current-range").textContent=schoolName();$("countdown").textContent="—";$("countdown-copy").textContent="No active countdown";$("live-progress").style.width="0";$("next-period").textContent=data.nextSchool?.title||"Check back soon";$("next-time").textContent=data.nextSchool?.date||"—";$("snapshot-progress").textContent="Not in session";setDial(0,"—","No active class","NEXT BELL");return}
  const now=p.minutes+p.seconds/60,current=s.find(x=>now>=mins(x.start)&&now<mins(x.end)),next=s.find(x=>mins(x.start)>now),dayPct=Math.min(100,Math.max(0,(now-mins(s[0].start))/(mins(s.at(-1).end)-mins(s[0].start))*100));
  $("snapshot-progress").textContent=`${Math.round(dayPct)}% complete`;
  if(current){const sec=Math.max(0,Math.round((mins(current.end)-now)*60)),pct=(now-mins(current.start))/(mins(current.end)-mins(current.start))*100,label=/Lunch/i.test(current.name)?"LUNCH ENDS IN":/Pack/i.test(current.name)?"PACK ENDS IN":"CLASS ENDS IN",n=s[s.indexOf(current)+1];$("live-mode").textContent="LIVE";$("current-eyebrow").textContent=label;$("current-period").textContent=current.name;$("current-range").textContent=`${time(current.start)} – ${time(current.end)}`;$("countdown").textContent=`${String(Math.floor(sec/60)).padStart(2,"0")}:${String(sec%60).padStart(2,"0")}`;$("countdown-copy").textContent="remaining";$("period-badge").textContent=current.name.match(/\d/)?.[0]||current.name[0];$("live-progress").style.width=`${pct}%`;$("next-period").textContent=n?.name||"Dismissal";$("next-time").textContent=n?time(n.start):time(current.end);setDial(pct,time(current.end),`${Math.ceil(sec/60)} min left`);return}
  if(next){const sec=Math.max(0,Math.round((mins(next.start)-now)*60)),before=now<mins(s[0].start),previous=[...s].reverse().find(x=>mins(x.end)<=now),start=previous?mins(previous.end):now,total=Math.max(1,mins(next.start)-start),pct=before?0:(now-start)/total*100;$("live-mode").textContent=before?"STARTING SOON":"PASSING";$("current-eyebrow").textContent=before?"SCHOOL STARTS IN":"CLASS STARTS IN";$("current-period").textContent=before?"First bell":next.name;$("current-range").textContent=`Starts at ${time(next.start)}`;$("countdown").textContent=`${String(Math.floor(sec/60)).padStart(2,"0")}:${String(sec%60).padStart(2,"0")}`;$("countdown-copy").textContent="until it begins";$("period-badge").textContent="→";$("live-progress").style.width=`${pct}%`;$("next-period").textContent=next.name;$("next-time").textContent=time(next.start);setDial(pct,time(next.start),before?"Before school":"Passing period","STARTS AT");return}
  $("live-mode").textContent="DONE";$("current-eyebrow").textContent="SCHOOL DAY COMPLETE";$("current-period").textContent="Dismissed";$("current-range").textContent=`Last bell at ${time(s.at(-1).end)}`;$("countdown").textContent="DONE";$("countdown-copy").textContent="See you next school day";$("period-badge").textContent="✓";$("live-progress").style.width="100%";$("next-period").textContent="Next school day";$("next-time").textContent="—";setDial(100,time(s.at(-1).end),"School is out","DISMISSED")
}
function render(){renderIdentity();renderTimeline();renderWeek();renderEvents();renderNowAdvanced()}
async function load(){
 const version=++requestVersion,selected=school;
 calendarVerified=false;$("source-status").textContent="Checking the official calendar · Published weekly times shown";
 data=buildData();render();
 try{
  const [r, or]=await Promise.all([
   fetch("/api/school-info?school="+selected,{cache:"no-store",signal:AbortSignal.timeout(20000)}).catch(()=>null),
   fetch("/api/bell/override",{cache:"no-store",signal:AbortSignal.timeout(10000)}).catch(()=>null)
  ]);
  if(version!==requestVersion)return;
  if(or && or.ok){
   try{
    const od=await or.json();
    bellOverride=od?.override||null;
   }catch{}
  }
  if(r && r.ok){
   const result=await r.json();
   if(version===requestVersion){
    calendarVerified=result.calendar_verified===true&&!result.stale;
    events=(result.events||[]).map(e=>({date:e.date,text:e.title}));
   }
  }
  data=buildData();render();
  if(bellOverride && bellOverride.date === pacific().date && (!bellOverride.school || bellOverride.school === school || school === 'woodcreek')){
   $("source-status").textContent=`Special schedule active · ${bellOverride.name || "Special day"}`;
  } else {
   $("source-status").textContent=calendarVerified?"Official calendar checked · Bell times verified":"Weekly bell times · Official calendar checked";
  }
 }catch{if(version===requestVersion){calendarVerified=false;$("source-status").textContent="Calendar unavailable · District breaks are included; check school announcements for special days.";$("announcement-text").textContent="Calendar unavailable — showing published weekly times."}}
}
$("school-select").innerHTML=Object.entries(SCHOOLS).map(([key,s])=>'<option value="'+key+'">'+s.name+'</option>').join("");
function switchSchool(s){
 if(!SCHOOLS[s]||s===school)return;
 school=s;
 if($("school-select"))$("school-select").value=school;
 remember("rjuhsd_school",school);
 lunch=saved("rjuhsd_lunch_"+school,"1");
 events=[];calendarCursor=null;scheduleMode="auto";includePeriod0=false;
 if($("include-period0"))$("include-period0").checked=false;
 applySchoolIdentity();
 data=buildData();
 render();
 load();
 loadWeather();
 try{const url=new URL(location.href);url.search="";url.pathname=school==="woodcreek"?"/":"/"+school+"/";history.replaceState(null,"",url)}catch{}
}
$("school-select").addEventListener("change",()=>switchSchool($("school-select").value));
document.addEventListener("click",e=>{const b=e.target.closest("[data-switch-school]");if(b){const s=b.dataset.switchSchool;if(s){switchSchool(s)}}});
$("schedule-mode").addEventListener("change",()=>{scheduleMode=$("schedule-mode").value;data=buildData();render()});
$("include-period0").addEventListener("change",()=>{includePeriod0=$("include-period0").checked;data=buildData();render()});
applySchoolIdentity();
document.querySelectorAll("[data-choose-lunch]").forEach(b=>b.addEventListener("click",()=>chooseLunch(b.dataset.chooseLunch)));["change-lunch","hero-change-lunch","brief-change-lunch"].forEach(id=>$(id).addEventListener("click",openLunch));$("calendar-prev").addEventListener("click",()=>moveCalendar(-1));$("calendar-next").addEventListener("click",()=>moveCalendar(1));$("calendar-today").addEventListener("click",()=>{const d=new Date(`${data.now?.iso||pacific().date}T12:00:00`);calendarCursor={year:d.getFullYear(),month:d.getMonth()};renderEvents()});$("weather-toggle").addEventListener("click",()=>{const open=$("weather-toggle").getAttribute("aria-expanded")!=="true";$("weather-toggle").setAttribute("aria-expanded",String(open));$("weather-details").hidden=!open;$("weather-widget").classList.toggle("expanded",open)});function syncRjuhsdTheme(){const isLight=document.documentElement.classList.contains("theme-light")||(window.__theme&&window.__theme.get()==="light");document.body.classList.toggle("dark",!isLight);const tc=document.querySelector('meta[name="theme-color"]');if(tc)tc.content=isLight?"#f7f4f4":"#0c0809";const b=$("theme-btn")||$("theme-toggle");if(b){const sun='<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.9 4.9l1.5 1.5m11.2 11.2 1.5 1.5M2 12h2m16 0h2M4.9 19.1l1.5-1.5M17.6 6.4l1.5-1.5"/></svg>';const moon='<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>';b.innerHTML=isLight?moon:sun;b.setAttribute("title",isLight?"Switch to dark theme":"Switch to light theme");b.setAttribute("aria-label",isLight?"Switch to dark theme":"Switch to light theme")}}syncRjuhsdTheme();window.addEventListener("themechange",syncRjuhsdTheme);const tb=$("theme-btn")||$("theme-toggle");if(tb){tb.addEventListener("click",()=>{const isLight=document.documentElement.classList.contains("theme-light");const next=isLight?"dark":"light";document.cookie="theme="+encodeURIComponent(next)+";path=/;max-age=31536000";if(window.__theme&&typeof window.__theme.apply==="function"){window.__theme.apply(next)}else{document.documentElement.classList.toggle("theme-light",next==="light");document.body.classList.toggle("dark",next==="dark")}syncRjuhsdTheme()})}load();loadWeather();setInterval(renderNowAdvanced,1000);setInterval(load,120000);setInterval(loadWeather,600000);
// Keep auth prompts out of the way for signed-in visitors. Controls remain
// hidden until the session check finishes so they never flash on screen.
try{
 const meCtl=new AbortController();const meTimer=setTimeout(()=>meCtl.abort(),6000);
 fetch("/api/me",{credentials:"include",signal:meCtl.signal}).then(r=>{if(!r.ok){r.text().catch(()=>{});return null;}return r.json();}).then(me=>{
  clearTimeout(meTimer);
  if(!me||!me.email){document.body.classList.add("auth-guest");document.querySelectorAll(".js-signin-link").forEach(a=>a.hidden=false);return}
  document.querySelectorAll(".js-signin-link").forEach(a=>a.remove());
 }).catch(()=>{clearTimeout(meTimer)});
}catch(e){}
})();
