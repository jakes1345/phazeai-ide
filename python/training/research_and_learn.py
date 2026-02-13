import sys
import os
import json
import time
import requests
from googlesearch import search
from bs4 import BeautifulSoup
from youtube_transcript_api import YouTubeTranscriptApi

# Configuration
TRAINING_DIR = "../data/training"
os.makedirs(TRAINING_DIR, exist_ok=True)

def learn_topic(topic, max_sources=5):
    print(f"🧠 AI Researching: {topic}...")
    knowledge = []

    # 1. Google Search for Articles
    print("  Typing search queries...")
    try:
        urls = list(search(f"{topic} tutorial", num_results=max_sources, advanced=True))
        for result in urls:
            if "youtube.com" in result.url:
                continue # Handle separately
                
            print(f"  reading: {result.title}")
            try:
                headers = {'User-Agent': 'Mozilla/5.0'}
                resp = requests.get(result.url, headers=headers, timeout=10)
                if resp.status_code == 200:
                    soup = BeautifulSoup(resp.content, 'html.parser')
                    # Extract content
                    content = ""
                    for p in soup.find_all(['p', 'code', 'pre', 'h1', 'h2', 'h3']):
                        content += p.get_text() + "\n"
                    
                    if len(content) > 500:
                        knowledge.append({
                            "input": f"Explain {topic} based on {result.title}",
                            "output": content[:4000], # Limit context
                            "metadata": {"source": result.url, "type": "web"}
                        })
            except Exception as e:
                print(f"    failed: {e}")
    except Exception as e:
        print(f"  Search error: {e}")

    # 2. YouTube Video Transcripts
    print("  Watching YouTube videos...")
    try:
        video_urls = list(search(f"{topic} youtube", num_results=3, advanced=True))
        for res in video_urls:
            if "youtube.com/watch" in res.url:
                try:
                    video_id = res.url.split("v=")[1].split("&")[0]
                    print(f"  transcribing: {res.title}")
                    transcript = YouTubeTranscriptApi.get_transcript(video_id)
                    full_text = " ".join([entry['text'] for entry in transcript])
                    
                    knowledge.append({
                        "input": f"Summarize video: {res.title} about {topic}",
                        "output": full_text[:4000],
                        "metadata": {"source": res.url, "type": "video"}
                    })
                except Exception:
                    pass
    except Exception:
        pass

    # 3. Save to Training Data
    if knowledge:
        filename = f"web_knowledge_{int(time.time())}.jsonl"
        filepath = os.path.join(TRAINING_DIR, filename)
        with open(filepath, 'w') as f:
            for entry in knowledge:
                f.write(json.dumps(entry) + "\n")
        print(f"✅ Learned {len(knowledge)} new concepts. Saved to {filename}")
        print("   The Training Pipeline will absorb this shortly.")
    else:
        print("❌ Found no readable information.")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 research_and_learn.py <topic>")
        sys.exit(1)
    
    learn_topic(sys.argv[1])
