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
const schoolPath = location.pathname.replace(/^\/|\/$/g,"");
const schoolHost = /(^|\.)rjuhsd\.school$/.test(location.hostname);
let school = (SCHOOLS[schoolPath] ? schoolPath : null) || new URLSearchParams(location.search).get("school") || saved("rjuhsd_school","woodcreek");
if(!SCHOOLS[school]) school="woodcreek";
let events=[],requestVersion=0,scheduleMode="auto",includePeriod0=false,calendarVerified=false,calendarState="checking",bellOverride=null;
function schoolName(){return SCHOOLS[school].name+" High School"}
function official(){return "https://"+school+".rjuhsd.us"}
function dayData(date){return window.RJUHSD_CALENDAR.resolve(school,date,events,date===pacific().date?scheduleMode:"auto",includePeriod0,bellOverride)}
/** Build calendar days independently of the displayed preview and selected lunch. */
function buildData(){
 const date=pacific().date,base=new Date(date+"T12:00:00Z");
 const weekdayName=base.toLocaleDateString("en-US",{weekday:"long",timeZone:"UTC"});
 const today=dayData(date),weekStart=new Date(base);
 weekStart.setUTCDate(weekStart.getUTCDate()-((weekStart.getUTCDay()+6)%7));
 const week=Array.from({length:5},(_,i)=>{
  const d=new Date(weekStart);d.setUTCDate(d.getUTCDate()+i);
  const key=d.toISOString().slice(0,10),v=dayData(key),blocks=blocksForDay(v);
  return {...v,weekdayName:d.toLocaleDateString("en-US",{weekday:"long",timeZone:"UTC"}),isToday:key===date,short:v.title,firstBell:blocks[0]?.start,lastBell:blocks.at(-1)?.end};
 });
 // Search across district breaks without inventing times outside the verified year.
 let nextSchool=null;
 for(let offset=1;offset<=45;offset++){
  const d=new Date(base);d.setUTCDate(d.getUTCDate()+offset);
  const key=d.toISOString().slice(0,10);
  const v=window.RJUHSD_CALENDAR.resolve(school,key,events,"auto",includePeriod0,bellOverride);
  const blocks=blocksForDay(v);
  if(v.inSession&&blocks.length){nextSchool={...v,firstBell:blocks[0].start,lastBell:blocks.at(-1).end,daysAway:offset};break}
 }
 return {now:{iso:date,weekdayName},today,week,nextSchool,officialEvents:events,upcoming:[],menu:{}};
}
function blocksForDay(day){
 if(day?.oneLunch)return day.lunch1||day.lunch2||[];
 return (lunch==="2"?day?.lunch2:day?.lunch1)||[];
}
function applySchoolIdentity(){
 document.body.dataset.school=school;
 const brand=window.RJUHSD_SCHOOL_DATA[school];
 document.documentElement.style.setProperty("--school-primary",brand.primary);
 document.documentElement.style.setProperty("--school-secondary",brand.secondary);
 document.querySelectorAll('img[src*="/rjuhsd-assets/"]:not(.brand-logo img):not(.site-logo):not(.district-school-logo)').forEach(img=>{img.src=brand.logo;img.alt=img.closest('[aria-hidden="true"]')?"":schoolName()+" logo"});
 const fi=document.querySelector('link[rel="icon"]');if(fi)fi.href="/favicon.ico";
 const tc=document.querySelector('meta[name="theme-color"]');if(tc)tc.content=document.documentElement.classList.contains("theme-light")?"#f3f4f6":"#111215";
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
 document.querySelectorAll('a[href*="woodcreek.rjuhsd.us"]:not(#official-bells):not(.school-card-actions a)').forEach(a=>a.dataset.schoolLink="true");
 document.querySelectorAll("[data-school-link]").forEach(a=>{a.href=official()+"/"+SCHOOLS[school].calendar;if(a.querySelector(".tool-logo")){a.href=official();a.querySelector("strong").textContent="School website"}});
 document.querySelectorAll('a[href*="maps/search"]').forEach(a=>{a.href="https://www.google.com/maps/search/?api=1&query="+encodeURIComponent(schoolName());a.querySelector("small").textContent="Campus directions"});
 document.querySelector(".weather-place strong").textContent=SCHOOLS[school].name;
 const calendarSchool=document.querySelector(".calendar-toolbar small");if(calendarSchool)calendarSchool.textContent=schoolName();
 const agendaTitle=document.querySelector(".agenda-heading > span");if(agendaTitle)agendaTitle.textContent="Upcoming at "+SCHOOLS[school].name;
 const calendarDescription=document.querySelector("#events .section-title-row p:not(.section-label)");if(calendarDescription)calendarDescription.textContent="School dates and upcoming events.";

}

let lunch=saved("rjuhsd_lunch_"+school,"1")==="2"?"2":"1",data=buildData(),calendarCursor,calendarSelected=null;
function pacific(){const p=Object.fromEntries(new Intl.DateTimeFormat("en-US",{timeZone:"America/Los_Angeles",year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false}).formatToParts(new Date()).map(x=>[x.type,x.value]));return{date:`${p.year}-${p.month}-${p.day}`,minutes:(+p.hour%24)*60+(+p.minute),seconds:+p.second}}
function mins(v){if(!v)return 0;const[h,m]=v.split(":").map(Number);return h*60+m}
function time(v){if(!v)return"Not set";let[h,m]=v.split(":").map(Number),s=h>=12?"PM":"AM";h=h%12||12;return`${h}:${String(m).padStart(2,"0")} ${s}`}
function schedule(){return blocksForDay(data.today)}
function duration(a,b){const n=Math.max(0,mins(b)-mins(a));return`${Math.floor(n/60)}h ${n%60}m`}
function toast(msg){const e=$("toast");e.textContent=msg;e.classList.add("show");clearTimeout(toast.t);toast.t=setTimeout(()=>e.classList.remove("show"),2200)}
let lunchReturnFocus=null,lunchPreviousOverflow="";
function openLunch(){
 const combined=!!data.today?.oneLunch,m=$("onboarding");
 lunchReturnFocus=document.activeElement;lunchPreviousOverflow=document.body.style.overflow;
 $("onboarding-title").textContent=combined?"Today uses one lunch":"Choose your lunch";
 $("onboarding-description").textContent=combined?"Today follows the combined lunch schedule. Choose your usual lunch for other days.":"Your lunch changes Period 3 and the bell countdown.";
 m.querySelectorAll("[data-choose-lunch]").forEach(button=>button.setAttribute("aria-pressed",String(button.dataset.chooseLunch===lunch)));
 m.hidden=false;document.body.style.overflow="hidden";
 (m.querySelector(`[data-choose-lunch="${lunch}"]`)||m.querySelector("button"))?.focus();
}
function closeLunch(){
 $("onboarding").hidden=true;document.body.style.overflow=lunchPreviousOverflow;
 if(lunchReturnFocus?.isConnected)lunchReturnFocus.focus();
}
function chooseLunch(value){
 if(value!=="1"&&value!=="2")return;
 lunch=value;remember("rjuhsd_lunch_"+school,value);remember("rjuhsd_onboarded_"+school,"yes");
 closeLunch();data=buildData();render();toast(`Lunch ${value} selected`);
}
// Keep keyboard navigation within the open dialog; restore the invoking control on close.
document.addEventListener("keydown",event=>{
 const modal=$("onboarding");if(modal.hidden)return;
 if(event.key==="Escape"){event.preventDefault();closeLunch();return}
 if(event.key!=="Tab")return;
 const controls=[...modal.querySelectorAll('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),[tabindex="0"]')].filter(element=>!element.hidden&&element.getClientRects().length);
 const first=controls[0],last=controls.at(-1);if(!first)return;
 if(event.shiftKey&&(document.activeElement===first||!modal.contains(document.activeElement))){event.preventDefault();last.focus()}
 else if(!event.shiftKey&&(document.activeElement===last||!modal.contains(document.activeElement))){event.preventDefault();first.focus()}
});
$("onboarding").addEventListener("click",event=>{if(event.target.classList.contains("onboarding-backdrop")||event.target.closest("#close-lunch"))closeLunch()});

function renderIdentity(){const iso=data.now?.iso||pacific().date,d=new Date(`${iso}T12:00:00`),combined=!!data.today?.oneLunch;$("hero-date").dateTime=iso;$("hero-weekday").textContent=data.now?.weekdayName||d.toLocaleDateString("en-US",{weekday:"long"});$("hero-date-day").textContent=String(d.getDate()).padStart(2,"0");$("hero-date-month").textContent=d.toLocaleDateString("en-US",{month:"long"});$("hero-date-year").textContent=d.getFullYear();$("date-month").textContent=d.toLocaleDateString("en-US",{month:"short"}).toUpperCase();$("date-day").textContent=String(d.getDate()).padStart(2,"0");$("date-year").textContent=d.getFullYear();$("hero-summary").textContent=data.today?.blurb||data.today?.event||"Regular weekly schedule.";$("header-lunch").textContent=combined?"Combined lunch":lunch?`Lunch ${lunch}`:"Choose lunch";$("brief-lunch").textContent=combined?"COMBINED LUNCH":`LUNCH ${lunch||"Not set"}`;$("schedule-note").textContent=combined?"Today uses a combined lunch.":`Showing times for Lunch ${lunch||"Not set"}.`;$("snapshot-type").textContent=(data.today?.title||"Today").replace(" schedule","");}
let timelineMinute="";
function renderTimeline(){
 const blocks=schedule(),now=pacific(),live=now.date===data.now?.iso&&!data.today?.manual;
 const minute=now.minutes+now.seconds/60;
 timelineMinute=`${now.date}|${now.minutes}`;
 $("timeline").setAttribute("role","list");
 $("timeline").setAttribute("aria-label",data.today?.manual?"Schedule preview":"Today's bell schedule");
 if(!data.today?.inSession||!blocks.length){
  $("timeline").innerHTML=`<div class="timeline-empty" role="listitem"><span class="timeline-empty-icon" aria-hidden="true">${data.today?.unavailable?"?":"✓"}</span><strong>${safe(data.today?.unavailable?"Schedule unavailable":data.today?.event||"No school today")}</strong><p>${safe(data.today?.unavailable?"Check the official school calendar for bell times.":nextSchoolDescription())}</p></div>`;
  ["snapshot-first","snapshot-last","day-length","period-count","pack-placement"].forEach((id,i)=>$(id).textContent=i?"Not set":data.today?.unavailable?"Unverified":"No school");
  return;
 }
 const nextIndex=blocks.findIndex(block=>mins(block.start)>minute);
 $("timeline").innerHTML=blocks.map((block,index)=>{
  const start=mins(block.start),end=mins(block.end),active=live&&minute>=start&&minute<end,passed=live&&minute>=end;
  const percent=active?clampPercent((minute-start)/Math.max(1,end-start)*100):passed?100:0;
  const classes=["timeline-row",active?"active":"",passed?"passed":"",live&&index===nextIndex?"upcoming":"",/lunch/i.test(block.name)?"is-lunch":""].filter(Boolean).join(" ");
  return `<div class="${classes}" role="listitem"${active?' aria-current="step"':""} style="--period-progress:${percent.toFixed(2)}%"><span class="time">${time(block.start).replace(" ","<small>")}<\/small></span><i class="timeline-dot" aria-hidden="true">${passed?"✓":""}</i><span class="period-name">${safe(block.name)}${active?'<b class="now-chip">Now</b>':""}<small class="period-duration">${end-start} min</small></span><span class="period-meta"><time>${time(block.end)}</time><small>${active?"Ends at":"End time"}</small></span></div>`;
 }).join("");
 $("snapshot-first").textContent=time(blocks[0].start);$("snapshot-last").textContent=time(blocks.at(-1).end);
 $("day-length").textContent=duration(blocks[0].start,blocks.at(-1).end);
 $("period-count").textContent=`${blocks.filter(block=>/^Period \d+$/.test(block.name)).length} classes`;
 const advisory=blocks.find(block=>/Pack|Roar|Titan|Panther|Intervention/i.test(block.name));
 $("pack-placement").textContent=advisory?time(advisory.start):"None today";
}
function renderWeek(){const s=schedule(),fallback=[{weekdayName:data.now?.weekdayName||"Today",date:data.now?.iso,short:data.today?.title,isToday:true,inSession:data.today?.inSession,firstBell:s[0]?.start,lastBell:s.at(-1)?.end}],week=data.week?.length?data.week:fallback;$("week-grid").innerHTML=week.map(x=>{const d=x.date?new Date(`${x.date}T12:00:00`):null;return`<article class="day-card${x.isToday?" today":""}${!x.inSession?" off":""}"><div class="day-top"><span>${x.weekdayName}</span>${x.isToday?'<b class="today-pill">TODAY</b>':""}</div><span class="day-date">${d?d.toLocaleDateString("en-US",{month:"short",day:"numeric"}):""}</span><p>${safe(x.event||x.short||"Regular schedule")}</p><strong>${x.inSession?`${time(x.firstBell)} to ${time(x.lastBell)}`:x.unavailable?"Unverified":"No school"}</strong></article>`}).join("")}
function safe(value){return String(value??"").replace(/[&<>"']/g,x=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[x])}
function eventData(){const official=(data.officialEvents||[]).map(x=>({...x,source:schoolName()})),officialDates=new Set(official.map(x=>x.date)),scheduleEvents=(data.upcoming||[]).filter(x=>!officialDates.has(x.date)).map(x=>({date:x.date,text:x.text,source:"Schedule"})),seen=new Set;return[...official,...scheduleEvents].filter(x=>x.date&&x.text&&!seen.has(`${x.date}|${x.text.toLowerCase()}`)&&seen.add(`${x.date}|${x.text.toLowerCase()}`)).sort((a,b)=>a.date.localeCompare(b.date))}
function renderEvents(){
 const events=eventData(),today=data.now?.iso||pacific().date,todayDate=new Date(`${today}T12:00:00Z`);
 if(!calendarCursor)calendarCursor={year:todayDate.getUTCFullYear(),month:todayDate.getUTCMonth()};
 const monthStart=new Date(Date.UTC(calendarCursor.year,calendarCursor.month,1)),first=monthStart.getUTCDay();
 const days=new Date(Date.UTC(calendarCursor.year,calendarCursor.month+1,0)).getUTCDate(),byDate=new Map;
 events.forEach(event=>{const list=byDate.get(event.date)||[];list.push(event);byDate.set(event.date,list)});
 $("calendar-title").textContent=monthStart.toLocaleDateString("en-US",{timeZone:"UTC",month:"long",year:"numeric"});
 const cells=[];
 for(let i=0;i<42;i++){
  const day=i-first+1;
  if(day<1||day>days){cells.push('<span class="outside" aria-hidden="true"></span>');continue}
  const key=`${calendarCursor.year}-${String(calendarCursor.month+1).padStart(2,"0")}-${String(day).padStart(2,"0")}`;
  const items=byDate.get(key)||[],labels=items.map(event=>event.text).join(", ");
  const classes=[key===today?"today":"",items.length?"has-event":"",key===calendarSelected?"selected":""].filter(Boolean).join(" ");
  const dateLabel=new Date(`${key}T12:00:00Z`).toLocaleDateString("en-US",{month:"long",day:"numeric",year:"numeric",timeZone:"UTC"});
  cells.push(`<button type="button" class="${classes}" data-calendar-date="${key}" aria-pressed="${key===calendarSelected}"${key===today?' aria-current="date"':""} aria-label="${safe(dateLabel)}${labels?`: ${safe(labels)}`:". No events posted"}" title="${safe(labels||"No events posted")}"><span>${day}</span>${items.length?`<i>${items.length}</i>`:""}</button>`);
 }
 $("calendar-days").innerHTML=cells.join("");
 const visible=calendarSelected?(byDate.get(calendarSelected)||[]):events.filter(event=>event.date>=today).slice(0,6);
 const heading=document.querySelector(".agenda-heading > span");
 if(heading)heading.textContent=calendarSelected?new Date(`${calendarSelected}T12:00:00Z`).toLocaleDateString("en-US",{weekday:"long",month:"short",day:"numeric",timeZone:"UTC"}):"Upcoming at "+SCHOOLS[school].name;
 $("event-list").innerHTML=visible.length?visible.map(event=>{
  const d=new Date(`${event.date}T12:00:00Z`);
  return `<li><time datetime="${event.date}"><span>${d.toLocaleDateString("en-US",{month:"short",timeZone:"UTC"}).toUpperCase()}</span><strong>${String(d.getUTCDate()).padStart(2,"0")}</strong></time><span><small>${d.toLocaleDateString("en-US",{weekday:"long",timeZone:"UTC"})}</small><strong>${safe(event.text)}</strong></span></li>`;
 }).join(""):`<li class="agenda-empty"><strong>${calendarSelected?"No events on this date.":"No upcoming events posted."}</strong><span>${calendarSelected?"Choose another date or return to Today.":"Check the official calendar for updates."}</span></li>`;
 const next=events.find(event=>event.date>=today);
 $("announcement-text").textContent=next?`Next up: ${next.text} · ${new Date(`${next.date}T12:00:00Z`).toLocaleDateString("en-US",{weekday:"short",month:"short",day:"numeric",timeZone:"UTC"})}`:(data.today?.blurb||"Published weekly schedule.");
}
function moveCalendar(offset){
 const d=new Date(Date.UTC(calendarCursor.year,calendarCursor.month+offset,1));
 calendarCursor={year:d.getUTCFullYear(),month:d.getUTCMonth()};calendarSelected=null;renderEvents();
}
$("calendar-days").addEventListener("click",event=>{
 const button=event.target.closest("[data-calendar-date]");if(!button)return;
 const date=button.dataset.calendarDate;calendarSelected=calendarSelected===date?null:date;
 renderEvents();$("calendar-days").querySelector(`[data-calendar-date="${date}"]`)?.focus();
});
function weatherInfo(code){if(code===0)return["☀","Clear"];if(code<=2)return["⛅","Partly cloudy"];if(code===3)return["☁","Cloudy"];if(code<=48)return["≋","Foggy"];if(code<=67||code>=80&&code<=82)return["🌧","Rain"];if(code<=77)return["❄","Snow"];if(code>=95)return["⛈","Thunderstorms"];return["☀","Fair"]}
function renderWeather(w){const current=w.current||{},daily=w.daily||{},info=weatherInfo(Number(current.weather_code));$("weather-symbol").textContent=info[0];$("weather-condition").textContent=info[1];$("weather-temperature").textContent=`${Math.round(current.temperature_2m)}°`;$("weather-feels").textContent=`${Math.round(current.apparent_temperature)}°`;$("weather-wind").textContent=`${Math.round(current.wind_speed_10m)} mph`;$("weather-range").textContent=`${Math.round(daily.temperature_2m_max?.[0])}° / ${Math.round(daily.temperature_2m_min?.[0])}°`;$("weather-forecast").innerHTML=(daily.time||[]).slice(0,7).map((date,i)=>{const d=new Date(`${date}T12:00:00`),day=i===0?"Today":d.toLocaleDateString("en-US",{weekday:"short"}),dayInfo=weatherInfo(Number(daily.weather_code?.[i]));return`<div><span>${day}</span><i aria-hidden="true">${dayInfo[0]}</i><strong>${Math.round(daily.temperature_2m_max?.[i])}°</strong><small>${Math.round(daily.temperature_2m_min?.[i])}°</small></div>`}).join("");$("weather-updated").textContent=`Updated ${new Intl.DateTimeFormat("en-US",{hour:"numeric",minute:"2-digit"}).format(new Date())} · ${SCHOOLS[school].name}, CA`}
async function loadWeather(){
 const selected=school,s=SCHOOLS[selected];
 try{
 const r=await fetch("https://api.open-meteo.com/v1/forecast?latitude="+s.lat+"&longitude="+s.lon+"&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&temperature_unit=fahrenheit&wind_speed_unit=mph&timezone=America%2FLos_Angeles&forecast_days=7",{signal:AbortSignal.timeout(10000)});
 if(!r.ok){r.text().catch(()=>{});throw Error();}const w=await r.json();if(selected!==school)return;if(!Number.isFinite(w.current?.temperature_2m))throw Error();renderWeather(w);
 }catch{if(selected===school){$("weather-condition").textContent="Forecast unavailable";$("weather-updated").textContent="Weather service is temporarily unavailable"}}
}
function clampPercent(value){return Math.min(100,Math.max(0,Number.isFinite(value)?value:0))}
function setDial(percent,end,context,label="Ends at"){
 const value=clampPercent(percent),ring=$("countdown-ring");
 ring.style.strokeDashoffset=String(326.73*(1-value/100));
 ring.closest(".countdown-dial").dataset.progress=`${Math.round(value)}%`;
 $("countdown-percent").textContent=`${Math.round(value)}%`;
 $("countdown-end").previousElementSibling.textContent=label;
 $("countdown-end").textContent=end||"Not set";$("countdown-context").textContent=context||"Not set";
}
function nextSchoolDescription(){
 if(!data.nextSchool)return "Check the official calendar for the next school day.";
 const date=new Date(data.nextSchool.date+"T12:00:00Z");
 return `Next school day: ${date.toLocaleDateString("en-US",{weekday:"long",month:"short",day:"numeric",timeZone:"UTC"})} at ${time(data.nextSchool.firstBell)}.`;
}
function showNextSchool(){
 document.querySelector(".live-next > span").textContent="Next school day";
 const next=data.nextSchool;
 $("next-period").textContent=next?new Date(next.date+"T12:00:00Z").toLocaleDateString("en-US",{weekday:"long",month:"short",day:"numeric",timeZone:"UTC"}):"Check official calendar";
 $("next-time").textContent=next?time(next.firstBell):"Not set";
}
function showNextPeriod(name,start){
 document.querySelector(".live-next > span").textContent="Up next";
 $("next-period").textContent=name;$("next-time").textContent=time(start);
}
// Bell times and the Pacific clock have whole-second precision; avoid fractional-minute rounding.
function showCountdown(seconds,copy){
 const remaining=Math.max(0,Math.round(seconds));
 $("countdown").textContent=`${String(Math.floor(remaining/60)).padStart(2,"0")}:${String(remaining%60).padStart(2,"0")}`;
 $("countdown").setAttribute("aria-label",`${Math.floor(remaining/60)} minutes ${remaining%60} seconds ${copy}`);
 $("countdown-copy").textContent=copy;
}
/** One clock drives the timer and schedule states, including minute boundaries and midnight. */
function renderNowAdvanced(){
 const card=document.querySelector(".live-card"),p=pacific(),blocks=schedule();
 const clock=new Intl.DateTimeFormat("en-US",{timeZone:"America/Los_Angeles",hour:"numeric",minute:"2-digit"}).format(new Date());
 $("live-clock").textContent=clock;$("header-clock").textContent=clock;
 // Update elapsed rows at each minute and exactly when a bell changes the active row.
 if(timelineMinute!==`${p.date}|${p.minutes}`)renderTimeline();
 const activeRow=$("timeline").querySelector(".timeline-row.active");
 if(activeRow){
  const block=blocks.find(block=>p.minutes>=mins(block.start)&&p.minutes<mins(block.end));
  if(block)activeRow.style.setProperty("--period-progress",`${clampPercent((p.minutes+p.seconds/60-mins(block.start))/Math.max(1,mins(block.end)-mins(block.start))*100)}%`);
 }
 if(p.date!==data.now?.iso&&!load.pending){load.pending=true;load().finally(()=>{load.pending=false})}
 if(data.today?.manual){
  card.dataset.countdownState="preview";$("live-mode").textContent="Preview";
  $("current-eyebrow").textContent="Schedule preview";$("current-period").textContent=data.today.title.replace(" schedule","");
  $("current-range").textContent="Choose Automatic to return to today's bells";$("period-badge").textContent="";
  $("countdown").textContent="Not set";$("countdown-copy").textContent="Manual schedule preview";
  $("snapshot-progress").textContent="Preview";$("live-progress").style.width="0%";
  setDial(0,"Not set","Preview");showNextSchool();return;
 }
 if(!data.today?.inSession||p.date!==data.now?.iso||!blocks.length){
  card.dataset.countdownState="off";$("live-mode").textContent="Today";
  $("current-eyebrow").textContent=data.today?.unavailable?"Calendar check":"School is out";
  $("current-period").textContent=data.today?.unavailable?"Check school calendar":"No school today";
  $("current-range").textContent=data.today?.event||schoolName();$("period-badge").textContent=data.today?.unavailable?"?":"✓";
  $("countdown").textContent="Not set";$("countdown-copy").textContent="No active countdown";
  $("snapshot-progress").textContent=data.today?.unavailable?"Unverified":"No school";$("live-progress").style.width="0%";
  setDial(0,"Not set","No active class");showNextSchool();return;
 }
 const now=p.minutes+p.seconds/60,current=blocks.find(block=>now>=mins(block.start)&&now<mins(block.end));
 const next=blocks.find(block=>mins(block.start)>now);
 const first=mins(blocks[0].start),last=mins(blocks.at(-1).end),dayPct=clampPercent((now-first)/Math.max(1,last-first)*100);
 $("snapshot-progress").textContent=`${Math.floor(dayPct)}% complete`;
 if(current){
  card.dataset.countdownState="active";
  const seconds=mins(current.end)*60-p.minutes*60-p.seconds,percent=clampPercent((now-mins(current.start))/Math.max(1,mins(current.end)-mins(current.start))*100);
  const isLunch=/lunch/i.test(current.name),nextBlock=blocks[blocks.indexOf(current)+1];
  $("live-mode").textContent="Live";$("current-eyebrow").textContent=isLunch?"Lunch ends in":"Current period";
  $("current-period").textContent=current.name;$("current-range").textContent=`${time(current.start)} to ${time(current.end)}`;
  $("period-badge").textContent=current.name.match(/\d+/)?.[0]||(isLunch?"L":"•");
  showCountdown(seconds,isLunch?"until lunch ends":"until the next bell");
  $("live-progress").style.width=`${percent}%`;setDial(percent,time(current.end),`${Math.ceil(seconds/60)} min left`);
  showNextPeriod(nextBlock?.name||"Dismissal",nextBlock?.start||current.end);return;
 }
 if(next){
  const before=now<first,previous=[...blocks].reverse().find(block=>mins(block.end)<=now);
  const start=previous?mins(previous.end):first,span=Math.max(1,mins(next.start)-start);
  const percent=before?0:clampPercent((now-start)/span*100),seconds=mins(next.start)*60-p.minutes*60-p.seconds;
  card.dataset.countdownState=before?"before":"passing";$("live-mode").textContent=before?"Before school":"Passing period";
  $("current-eyebrow").textContent=before?"First bell":"Next class";$("current-period").textContent=before?"School starts in":"Passing period";
  $("current-range").textContent=`${next.name} starts at ${time(next.start)}`;$("period-badge").textContent="";
  showCountdown(seconds,before?"until the first bell":"until class begins");
  $("live-progress").style.width=`${percent}%`;setDial(percent,time(next.start),before?"Before school":"Passing period","Starts at");
  showNextPeriod(next.name,next.start);return;
 }
 card.dataset.countdownState="complete";$("live-mode").textContent="Dismissed";
 $("current-eyebrow").textContent="School day complete";$("current-period").textContent="School's out";
 $("current-range").textContent=`Dismissed at ${time(blocks.at(-1).end)}`;$("period-badge").textContent="✓";
 $("countdown").textContent="DONE";$("countdown-copy").textContent="School day complete";
 $("live-progress").style.width="100%";setDial(100,time(blocks.at(-1).end),"School is out","Dismissed");showNextSchool();
}
function renderFinalsCountdown(){const el=$("finals-countdown-text");if(!el)return;const today=data.now?.iso||pacific().date,match=eventData().find(x=>x.date>=today&&/final|semester/i.test(x.text));if(!match){el.textContent="No finals or semester date has been posted.";return}const days=Math.round((new Date(`${match.date}T12:00:00`)-new Date(`${today}T12:00:00`))/86400000);el.textContent=days<=0?`${safe(match.text)} is today!`:`${days} day${days===1?"":"s"} until ${safe(match.text)} (${new Date(`${match.date}T12:00:00`).toLocaleDateString("en-US",{month:"short",day:"numeric"})}).`}
/** Calendar checks confirm event availability, not a fresh verification of the bell-time document. */
function renderSourceStatus(){
 if(data.today?.manual){$("source-status").textContent="Manual preview · Published bell times";return}
 if(data.today?.type==="special"){$("source-status").textContent=`Special schedule active · ${data.today.title}`;return}
 $("source-status").textContent=calendarState==="checking"?"Checking the official calendar · Published weekly times shown":calendarVerified?"Official calendar checked · Published bell times":calendarState==="available"?"Published weekly times · Check school announcements":"Calendar unavailable · Published weekly times";
}
function render(){renderIdentity();renderTimeline();renderWeek();renderEvents();renderNowAdvanced();renderFinalsCountdown();renderSourceStatus()}
async function load(){
 const version=++requestVersion,selected=school;
 calendarVerified=false;calendarState="checking";
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
    if(version!==requestVersion)return;
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
  if(version!==requestVersion)return;
  calendarState=r?.ok?"available":"unavailable";
  data=buildData();render();
 }catch{if(version===requestVersion){calendarVerified=false;calendarState="unavailable";renderSourceStatus();$("announcement-text").textContent="Calendar unavailable. Showing published weekly times."}}

}
$("school-select").innerHTML=Object.entries(SCHOOLS).map(([key,s])=>'<option value="'+key+'">'+s.name+'</option>').join("");$("school-select").value=school;
function switchSchool(s){
 if(!SCHOOLS[s]||s===school)return;
 school=s;
 if($("school-select"))$("school-select").value=school;
 remember("rjuhsd_school",school);
 lunch=saved("rjuhsd_lunch_"+school,"1")==="2"?"2":"1";
 events=[];bellOverride=null;calendarCursor=null;calendarSelected=null;scheduleMode="auto";includePeriod0=false;
 if($("include-period0"))$("include-period0").checked=false;
 applySchoolIdentity();
 data=buildData();
 render();
 load();
 loadWeather();
 try{const url=new URL(location.href);url.search="";url.pathname=schoolHost?(school==="woodcreek"?"/":"/"+school+"/"):"/rjuhsd/";if(!schoolHost&&school!=="woodcreek")url.searchParams.set("school",school);history.replaceState(null,"",url)}catch{}
}
$("school-select").addEventListener("change",()=>switchSchool($("school-select").value));
document.addEventListener("click",e=>{const b=e.target.closest("[data-switch-school]");if(b){const s=b.dataset.switchSchool;if(s){e.preventDefault();switchSchool(s);document.getElementById("schedule-panel")?.scrollIntoView({block:"start"})}}});
$("schedule-mode").addEventListener("change",()=>{scheduleMode=$("schedule-mode").value;data=buildData();render()});
$("include-period0").addEventListener("change",()=>{includePeriod0=$("include-period0").checked;data=buildData();render()});
applySchoolIdentity();
document.querySelectorAll("[data-choose-lunch]").forEach(b=>b.addEventListener("click",()=>chooseLunch(b.dataset.chooseLunch)));["change-lunch","hero-change-lunch","brief-change-lunch"].forEach(id=>$(id).addEventListener("click",openLunch));$("calendar-prev").addEventListener("click",()=>moveCalendar(-1));$("calendar-next").addEventListener("click",()=>moveCalendar(1));$("calendar-today").addEventListener("click",()=>{const d=new Date(`${data.now?.iso||pacific().date}T12:00:00`);calendarCursor={year:d.getFullYear(),month:d.getMonth()};calendarSelected=null;renderEvents()});$("weather-toggle").addEventListener("click",()=>{const open=$("weather-toggle").getAttribute("aria-expanded")!=="true";$("weather-toggle").setAttribute("aria-expanded",String(open));$("weather-details").hidden=!open;$("weather-widget").classList.toggle("expanded",open)});function syncRjuhsdTheme(){const isLight=document.documentElement.classList.contains("theme-light")||(window.__theme&&window.__theme.get()==="light");document.body.classList.toggle("dark",!isLight);const tc=document.querySelector('meta[name="theme-color"]');if(tc)tc.content=isLight?"#f3f4f6":"#111215";const b=$("theme-btn")||$("theme-toggle");if(b){const sun='<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.9 4.9l1.5 1.5m11.2 11.2 1.5 1.5M2 12h2m16 0h2M4.9 19.1l1.5-1.5M17.6 6.4l1.5-1.5"/></svg>';const moon='<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>';b.innerHTML=isLight?moon:sun;b.setAttribute("title",isLight?"Switch to dark theme":"Switch to light theme");b.setAttribute("aria-label",isLight?"Switch to dark theme":"Switch to light theme")}}syncRjuhsdTheme();window.addEventListener("themechange",syncRjuhsdTheme);const tb=$("theme-btn")||$("theme-toggle");if(tb){tb.addEventListener("click",()=>{const isLight=document.documentElement.classList.contains("theme-light");const next=isLight?"dark":"light";document.cookie="theme="+encodeURIComponent(next)+";path=/;max-age=31536000";if(window.__theme&&typeof window.__theme.apply==="function"){window.__theme.apply(next)}else{document.documentElement.classList.toggle("theme-light",next==="light");document.body.classList.toggle("dark",next==="dark")}syncRjuhsdTheme()})}load();loadWeather();setInterval(renderNowAdvanced,1000);setInterval(load,120000);setInterval(loadWeather,600000);

// Both navigation bars reflect the section currently at the top of the viewport.
(function(){
 const links=[...document.querySelectorAll('.site-header nav a,.mobile-nav-dock a')];
 const targets=[...new Set(links.map(link=>link.hash))].map(hash=>document.querySelector(hash)).filter(Boolean);
 let queued=false;
 function update(){
  queued=false;
  let selected=targets[0],nearest=-Infinity;
  targets.forEach(target=>{
   const top=target.getBoundingClientRect().top;
   if(top<=150&&top>nearest){selected=target;nearest=top}
  });
  links.forEach(link=>{
   if(link.hash==='#'+selected?.id)link.setAttribute('aria-current','location');
   else link.removeAttribute('aria-current');
  });
 }
 window.addEventListener('scroll',()=>{if(!queued){queued=true;requestAnimationFrame(update)}},{passive:true});
 window.addEventListener('resize',update);update();
})();

// ── Study timer (focus/break interval timer, client-side only) ───────
(function(){
 const modeEl=$("study-timer-mode"),clockEl=$("study-timer-clock"),toggleBtn=$("study-timer-toggle"),resetBtn=$("study-timer-reset"),focusInput=$("study-timer-focus-len"),breakInput=$("study-timer-break-len");
 if(!modeEl||!clockEl||!toggleBtn||!resetBtn||!focusInput||!breakInput)return;
 let mode="focus",remaining=0,running=false,tickId=null;
 function fmt(sec){sec=Math.max(0,Math.round(sec));return `${String(Math.floor(sec/60)).padStart(2,"0")}:${String(sec%60).padStart(2,"0")}`}
 function lengthFor(m){const v=Number(m==="focus"?focusInput.value:breakInput.value);return(Number.isFinite(v)&&v>0?v:(m==="focus"?25:5))*60}
 function paint(){modeEl.textContent=mode==="focus"?"FOCUS":"BREAK";clockEl.textContent=fmt(remaining);toggleBtn.textContent=running?"Pause":"Start"}
 function switchMode(next){mode=next;remaining=lengthFor(mode);paint()}
 function pause(){running=false;if(tickId){clearInterval(tickId);tickId=null}paint()}
 function tick(){remaining-=1;if(remaining<=0){pause();switchMode(mode==="focus"?"break":"focus");return}paint()}
 function start(){if(running)return;running=true;tickId=setInterval(tick,1000);paint()}
 toggleBtn.addEventListener("click",()=>running?pause():start());
 resetBtn.addEventListener("click",()=>{pause();switchMode("focus")});
 [focusInput,breakInput].forEach(inp=>inp.addEventListener("change",()=>{if(!running)switchMode(mode)}));
 switchMode("focus");
})();
})();
