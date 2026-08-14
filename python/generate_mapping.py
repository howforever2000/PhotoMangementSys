"""生成 ImageNet-1k → 相册大类映射表（可人工后续调整 JSON）

用法: python generate_mapping.py
产出: models/imagenet_to_album.json
        {"mapping": {大类: [细类名...]}, "animal_sub": {子类: [细类名...]}, "meta": {...}}

归类规则（v2）：
  1. OVERRIDES：精确类名 → 大类，优先级最高（修历史子串误匹配）
  2. RULES：整词正则（\\b 边界）按列表顺序先匹配先得，杜绝
     "West Highland white terrier 被 highland 吸入风景" 这类错误
  3. 未命中 → other

大类（v2）：animal / food / plant_flower / architecture / sports /
           landscape_nature / text / vehicle / other
子类：animal_sub 将动物细分为 dog / cat / bird / 其他动物（按 ImageNet 索引区间，
     标准排序下 dog=151-268、domestic cat=281-285、bird=7-24/80-100/127-146）
"""
import json
import os
import re

HERE = os.path.dirname(__file__)

# ---------------------------------------------------------------------------
# 精确覆盖表（类名 → 大类）——优先级最高，用于修复歧义类
# ---------------------------------------------------------------------------
OVERRIDES = {
    # 历史误映射修复
    "West Highland white terrier": "animal",      # 曾被 highland 吸入风景
    "beach wagon": "vehicle",                     # 旅行车，曾被 beach 吸入风景
    "cliff dwelling": "architecture",             # 悬崖居所，是建筑
    "solar dish": "other",                        # 太阳能灶，是设备
    "coral reef": "landscape_nature",             # 珊瑚礁，是自然景观
    "school bus": "vehicle",                      # 曾被 school 吸入建筑
    # 交通工具兜底（车辆类量大，统一归 vehicle）
    "sports car": "vehicle", "convertible": "vehicle", "limousine": "vehicle",
    "minivan": "vehicle", "racer": "vehicle", "streetcar": "vehicle",
    "fireboat": "vehicle", "airliner": "vehicle", "cab": "vehicle",
    "jeep": "vehicle", "minibus": "vehicle", "police van": "vehicle",
    "recreational vehicle": "vehicle", "tow truck": "vehicle",
    "trailer truck": "vehicle", "garbage truck": "vehicle",
    "pickup": "vehicle", "ambulance": "vehicle", "beach buggy": "vehicle",
    "forklift": "vehicle", "tractor": "vehicle", "harvester": "vehicle",
    "snowplow": "vehicle", "snowmobile": "vehicle", "go-kart": "vehicle",
    "golfcart": "vehicle", "moped": "vehicle", "motor scooter": "vehicle",
    "mountain bike": "vehicle", "tandem": "vehicle", "bicycle-built-for-two": "vehicle",
    "unicycle": "vehicle", "tricycle": "vehicle", "freight car": "vehicle",
    "passenger car": "vehicle", "bullet train": "vehicle", "steam locomotive": "vehicle",
    "electric locomotive": "vehicle", "subway train": "vehicle",
    "speedboat": "vehicle", "lifeboat": "vehicle", "gondola": "vehicle",
    "liner": "vehicle", "container ship": "vehicle", "wreck": "vehicle",
    "pirate": "vehicle", "schooner": "vehicle", "trimaran": "vehicle",
    "fire truck": "vehicle",
    "pot": "other", "cup": "other",  # 锅/杯单独成器，不属食物
    "warplane": "vehicle", "airship": "vehicle", "space shuttle": "vehicle",
    "catamaran": "vehicle", "yawl": "vehicle", "paddlewheel": "vehicle",
    "dogsled": "vehicle", "horse cart": "vehicle", "oxcart": "vehicle",
    "baby buggy": "vehicle", "shopping cart": "other", "wheelchair": "vehicle",
    # 与人相关的服饰/角色 → other（交给检测路判人像，分类不误导）
    "groom": "other", "ballplayer": "other", "military uniform": "other",
    "scuba diver": "other", "suit": "other", "gown": "other",
    "jersey": "other", "Windsor tie": "other", "bow tie": "other",
    "cowboy hat": "other", "sombrero": "other", "mortarboard": "other",
    "academic gown": "other", "abaya": "other", "kimono": "other",
    "pajama": "other", "sweatshirt": "other", "cardigan": "other",
    "trench coat": "other", "poncho": "other", "sarong": "other",
    "stole": "other", "fur coat": "other", "wig": "other",
    "maillot": "other", "bikini": "other", "miniskirt": "other",
    "diaper": "other", "sock": "other", "loafer": "other",
    "sandal": "other", "clog": "other", "cowboy boot": "other",
    "running shoe": "sports", "crash helmet": "sports", "football helmet": "sports",
    "ski mask": "other", "bathing cap": "sports", "swimming trunks": "sports",
    "military cap": "other", "bearskin": "other", "bonnet": "other",
    "bathing trunks": "sports",
    # 场景补强：ImageNet 中的少量真场景类
    "alp": "landscape_nature", "cliff": "landscape_nature",
    "geyser": "landscape_nature", "lakeside": "landscape_nature",
    "promontory": "landscape_nature", "sandbar": "landscape_nature",
    "seashore": "landscape_nature", "valley": "landscape_nature",
    "volcano": "landscape_nature", "coral fungus": "plant_flower",
    "coral": "animal",
    # 文本类补强
    "book jacket": "text", "comic book": "text", "crossword puzzle": "text",
    "menu": "text", "packet": "text", "web site": "text",
}

# ---------------------------------------------------------------------------
# 同名类按索引精确区分（crane=134 鸟 / 517 机械起重机）
# ---------------------------------------------------------------------------
INDEX_OVERRIDES = {134: "animal", 517: "other"}


# ---------------------------------------------------------------------------
# 整词正则规则（先匹配先得）；注意顺序即优先级
# ---------------------------------------------------------------------------
def _w(words):
    """单词列表 → 整词正则（避免子串误匹配）"""
    return re.compile(r"\b(" + "|".join(re.escape(w) for w in words) + r")\b", re.I)


RULES = [
    ("animal", _w([
        "dog", "puppy", "cat", "kitten", "bird", "hen", "cock", "rooster", "chick",
        "quail", "goose", "duck", "drake", "ostrich", "emu", "rhea", "kiwi",
        "frog", "toad", "salamander", "newt", "axolotl",
        "turtle", "terrapin", "lizard", "iguana", "alligator", "crocodile",
        "snake", "viper", "cobra", "python", "boa", "gecko", "dinosaur",
        "lion", "tiger", "leopard", "cheetah", "jaguar", "cougar", "lynx",
        "bear", "panda", "elephant", "rhinoceros", "hippopotamus", "zebra",
        "giraffe", "camel", "llama", "alpaca", "bison", "buffalo", "ox",
        "cow", "bull", "pig", "boar", "hog", "sheep", "ram", "ewe", "lamb",
        "goat", "ibex", "deer", "elk", "moose", "reindeer", "antelope",
        "gazelle", "impala", "hartebeest", "fox", "wolf", "hyena", "jackal",
        "coyote", "dingo", "dhole", "raccoon", "weasel", "mink", "ferret",
        "badger", "otter", "skunk", "polecat", "mole", "porcupine", "beaver",
        "hamster", "guinea pig", "rabbit", "hare", "kangaroo", "wallaby",
        "koala", "wombat", "opossum", "sloth", "armadillo", "anteater",
        "platypus", "echidna", "hedgehog", "squirrel", "chipmunk", "rat",
        "mouse", "vole", "marmot", "beaver", "monkey", "ape", "gorilla",
        "chimpanzee", "orangutan", "gibbon", "siamang", "baboon", "macaque",
        "langur", "colobus", "marmoset", "capuchin", "titi", "indri",
        "guenon", "patas", "lemur", "bat", "whale", "dolphin", "porpoise",
        "seal", "walrus", "manatee", "dugong", "shark", "ray", "eel",
        "fish", "salmon", "trout", "sturgeon", "gar", "lionfish", "puffer",
        "crab", "lobster", "crayfish", "shrimp", "prawn", "hermit crab",
        "snail", "slug", "spider", "tarantula", "tick", "scorpion",
        "centipede", "millipede", "beetle", "weevil", "ladybug", "firefly",
        "butterfly", "moth", "caterpillar", "grasshopper", "cricket",
        "cockroach", "mantis", "dragonfly", "damselfly", "fly", "bee",
        "wasp", "hornet", "ant", "termite", "cicada", "leafhopper",
        "jellyfish", "starfish", "sea urchin", "sea cucumber", "anemone",
        "coral", "conch", "chambered nautilus", "nautilus", "squid", "octopus",
        "clam", "oyster", "mussel", "scallop", "barnacle", "isopod",
        "toucan", "hornbill", "hummingbird", "jacamar", "lorikeet", "macaw",
        "parrot", "cockatoo", "budgerigar", "parakeet", "lovebird", "canary",
        "penguin", "puffin", "gannet", "pelican", "albatross", "cormorant",
        "flamingo", "stork", "heron", "egret", "ibis", "spoonbill", "crane",
        "bustard", "bittern", "limpkin", "gallinule", "coot", "turnstone",
        "sandpiper", "redshank", "dowitcher", "oystercatcher", "avocet",
        "grouse", "ptarmigan", "peacock", "partridge", "pheasant", "turkey",
        "vulture", "condor", "eagle", "hawk", "harrier", "kite", "osprey",
        "falcon", "kestrel", "owl", "swift", "swallow", "martin", "lark",
        "thrush", "robin", "blackbird", "starling", "myna", "crow", "raven",
        "rook", "jackdaw", "magpie", "jay", "chough", "wren", "tit",
        "nuthatch", "warbler", "finch", "sparrow", "bunting", "cardinal",
        "grosbeak", "tanager", "oriole", "bulbul", "shrike", "flycatcher",
        "nightingale", "whippoorwill", "woodpecker", "kingfisher", "bee eater",
        "roller", "hoopoe", "cuckoo", "coucal", "roadrunner", "dove",
        "pigeon", "brambling", "junco", "indigo bunting",
        "goldfinch", "siskin", "redpoll", "crossbill", "towhee", "wagtail",
        "pipit", "dipper", "ouzel", "vireo", "waxwing", "mockingbird",
        "catbird", "thrasher", "wheatear", "redstart", "nightjar",
        "loggerhead", "water ouzel", "kite", "bald eagle", "African grey",
        "snowbird", "prairie chicken", "ruffed grouse", "black grouse",
        "black swan", "goose", "drake", "red-breasted merganser", "loon",
        "grebe", "fulmar", "shearwater", "petrel", "guillemot", "puffin",
        "auk", "skua", "gull", "tern", "jaeger", "kittiwake",
        "African hunting dog", "Arctic fox", "grey fox", "kit fox",
        "red fox", "red wolf", "white wolf", "timber wolf", "ice bear",
        "brown bear", "American black bear", "sloth bear", "sun bear",
        "polar bear", "giant panda", "lesser panda", "red panda",
        "wild boar", "warthog", "hippopotamus", "bighorn", "chamois",
        "oryx", "gazelle", "hartebeest", "wildebeest", "water buffalo",
        "bison", "yak", "ox", "bullock", "steer", "heifer", "calf",
        "colt", "foal", "stallion", "mare", "pony", "donkey", "mule",
        "burro", "ass", "zebra", "quagga", "tapir", "okapi",
        "proboscis monkey", "howler monkey", "spider monkey", "woolly monkey",
        "squirrel monkey", "tarsier", "aye-aye", "bushbaby", "loris",
        "patas", "guenon", "vervet", "macaque", "rhesus", "mandrill",
        "drill", "gibbon", "siamang", "orangutan", "chimpanzee", "bonobo",
        "gorilla", "baboon", "gelada", "hamadryas", "wallaby", "koala",
        "wombat", "tasmanian devil", "numbat", "quokka", "bandicoot",
        "bilby", "pademelon", "tree kangaroo", "wallaroo", "euro",
        "agama", "iguana", "chameleon", "gecko", "skink", "monitor",
        "komodo dragon", "gila monster", "horned lizard", "fence lizard",
        "anole", "whiptail", "blindworm", "slowworm", "glass lizard",
        "alligator", "crocodile", "caiman", "gharial", "tortoise",
        "terrapin", "box turtle", "mud turtle", "loggerhead", "leatherback",
        "hawksbill", "green turtle", "softshell", "sea snake", "king snake",
        "garter snake", "water snake", "vine snake", "night snake",
        "boa constrictor", "anaconda", "rock python", "indian cobra",
        "green mamba", "sea snake", "horned viper", "diamondback",
        "rattlesnake", "sidewinder", "copperhead", "cottonmouth",
        "fer-de-lance", "bushmaster", "puff adder", "asp", "adder",
        "trilobite", "harvestman", "tick", "centipede", "millipede",
        "daddy longlegs", "wolf spider", "garden spider", "orb weaver",
        "black widow", "tarantula", "scorpion", "whip scorpion",
        "vinegaroon", "sun spider", "wind scorpion", "booklouse",
        "silverfish", "bristletail", "springtail", "earwig", "termite",
        "louse", "flea", "bedbug", "stinkbug", "leafhopper", "aphid",
        "scale insect", "mealybug", "whitefly", "psyllid", "thrips",
        "lacewing", "antlion", "dobsonfly", "alderfly", "snakefly",
        "scorpionfly", "caddisfly", "stonefly", "mayfly", "dragonfly",
        "damselfly", "grasshopper", "locust", "cricket", "katydid",
        "walking stick", "leaf insect", "praying mantis", "cockroach",
        "beetle", "weevil", "firefly", "glowworm", "ladybird",
        "ground beetle", "longhorn beetle", "leaf beetle", "dung beetle",
        "rhinoceros beetle", "stag beetle", "click beetle", "soldier beetle",
        "blister beetle", "oil beetle", "butterfly", "moth", "sulphur butterfly",
        "cabbage butterfly", "admiral", "ringlet", "monarch", "viceroy",
        "fritillary", "hairstreak", "copper", "blue", "skipper",
        "swallowtail", "luna moth", "polyphemus moth", "cecropia moth",
        "promethea moth", "io moth", "sphinx moth", "hawk moth",
        "tiger moth", "tussock moth", "gypsy moth", "clothes moth",
        "bee", "bumblebee", "honeybee", "carpenter bee", "leafcutter bee",
        "wasp", "yellow jacket", "hornet", "paper wasp", "mud dauber",
        "ant", "army ant", "driver ant", "bulldog ant", "carpenter ant",
        "fire ant", "harvester ant", "leafcutter ant", "termite",
        "sawfly", "horntail", "ichneumon fly", "chalcid wasp",
        "tiger shark", "hammerhead", "great white shark", "whale shark",
        "basking shark", "mako", "porbeagle", "thresher", "wobbegong",
        "nurse shark", "zebra shark", "bull shark", "lemon shark",
        "blue shark", "manta ray", "stingray", "eagle ray", "skate",
        "sawfish", "guitarfish", "torpedo", "electric ray", "sturgeon",
        "paddlefish", "gar", "bowfin", "tarpon", "bonefish", "herring",
        "sardine", "anchovy", "shad", "salmon", "trout", "char",
        "whitefish", "grayling", "pike", "muskellunge", "pickerel",
        "smelt", "capelin", "lanternfish", "eelpout", "burbot",
        "cod", "haddock", "hake", "pollock", "whiting", "bass",
        "grouper", "snapper", "sea bream", "porgy", "grunt", "triggerfish",
        "filefish", "puffer", "porcupinefish", "boxfish", "trunkfish",
        "cowfish", "seahorse", "pipefish", "shrimpfish", "trumpetfish",
        "cornetfish", "flounder", "halibut", "sole", "plaice", "turbot",
        "tench", "goldfish", "carp", "koi", "barbel", "bream",
        "minnow", "dace", "chub", "roach", "rudd", "loach", "catfish",
        "bullhead", "electric eel", "knifefish", "elephantfish",
        "angelfish", "butterflyfish", "damselfish", "clownfish",
        "wrasse", "parrotfish", "goby", "blenny", "sculpin", "gunnel",
        "prickleback", "wolf fish", "wolffish", "ling", "grenadier",
        "rattail", "cutlassfish", "hairtail", "frostfish", "scabbardfish",
        "barracuda", "mullet", "silverside", "needlefish", "flying fish",
        "halfbeak", "sauries", "mackerel", "tuna", "bonito", "albacore",
        "bluefin", "yellowfin", "swordfish", "marlin", "sailfish",
        "spearfish", "sunfish", "opah", "oarfish", "ribbonfish",
        "deal fish", "tenpounder", "ladyfish", "bonefish", "milkfish",
        "beach salmon", "amberjack", "yellowtail", "jack", "scad",
        "pompano", "permit", "lookdown", "moonfish", "barracouta",
        "snoek", "gemfish", "warehou", "butterfish", "pomfret",
        "harvestfish", "rudderfish", "sea chub", "nibbler", "halfmoon",
        "opal eye", "zebrafish", "danio", "rasbora", "tetra", "barb",
        "gourami", "betta", "siamese fighting fish", "killifish",
        "livebearer", "guppy", "molly", "platy", "swordtail", "mollie",
        "cichlid", "discus", "oscar", "angelfish", "tilapia", "perch",
        "darter", "logperch", "walleye", "sauger", "sunfish", "bluegill",
        "pumpkinseed", "crappie", "rock bass", "warmouth", "mudminnow",
        "cavefish", "springfish", "poolfish", "killifish", "topminnow",
        "mosquitofish", " pupfish", "pupfish", "cuatro ojos", "four-eyed fish",
    ])),
    ("food", _w([
        "pizza", "hotdog", "hot dog", "hamburger", "cheeseburger", "sandwich",
        "burrito", "taco", "tortilla", "guacamole", "salsa", "potpie",
        "meat loaf", "mashed potato", "french loaf", "bagel", "pretzel",
        "dough", "bread", "biscuit", "muffin", "corn", "pancake", "waffle",
        "omelet", "egg", "ice cream", "ice lolly", "chocolate sauce",
        "trifle", "cheesecake", "cake", "cupcake", "gingerbread",
        "chocolate", "candy", "lollipop", "marshmallow", "caramel",
        "apple", "banana", "orange", "lemon", "lime", "strawberry",
        "raspberry", "blackberry", "cherry", "peach", "apricot", "plum",
        "pear", "grape", "fig", "pineapple", "mango", "papaya",
        "pomegranate", "watermelon", "cantaloup", "honeydew", "coconut",
        "acorn squash", "butternut squash", "zucchini", "cucumber",
        "pumpkin", "artichoke", "asparagus", "broccoli", "cauliflower",
        "cabbage", "cardoon", "bell pepper", "carrot", "potato",
        "onion", "garlic", "pepper", "tomato", "eggplant", "lettuce",
        "spinach", "corn", "spaghetti squash", "espresso", "cup",
        "beer", "wine", "eggnog", "consomme", "soup bowl", "red wine",
    ])),
    ("plant_flower", _w([
        "daisy", "rose", "sunflower", "tulip", "orchid", "lily",
        "rapeseed", "cardoon", "corn", "acorn", "buckeye", "hip",
        "flower", "blossom", "bouquet", "garland", "bonsai", "mushroom",
        "toadstool", "fungus", "agaric", "gyromitra", "stinkhorn",
        "earthstar", "hen-of-the-woods", "bolete", "coral fungus",
        "pinecone", "conifer", "pine", "spruce", "fir", "cedar",
        "cypress", "yew", "juniper", "hemlock", "redwood", "sequoia",
        "maple", "oak", "birch", "beech", "elm", "willow", "poplar",
        "aspen", "chestnut", "holly", "magnolia", "palm", "fern",
        "moss", "lichen", "bamboo", "cactus", "succulent", "vine",
        "wreath", "flowerpot",
    ])),
    ("architecture", _w([
        "palace", "castle", "fort", "citadel", "church", "cathedral",
        "monastery", "abbey", "temple", "mosque", "pagoda", "shrine",
        "library", "museum", "theater", "cinema", "stadium", "arena",
        "amphitheater", "aqueduct", "bridge", "viaduct", "dam",
        "lighthouse", "windmill", "water tower", "obelisk", "monument",
        "pyramid", "sphinx", "barn", "greenhouse", "hut", "cabin",
        "cottage", "mansion", "villa", "chateau", "school", "hospital",
        "hotel", "motel", "prison", "courthouse", "capitol", "embassy",
        "warehouse", "factory", "mill", "forge", "bakery", "grocery store",
        "shopping mall", "department store", "supermarket", "bookshop",
        "bookstore", "barbershop", "butcher shop", "confectionery",
        "shoe shop", "tobacco shop", "toyshop", "airport", "hangar",
        "dome", "vault", "tile roof", "thatch", "alp", "pier", "dock",
        "wharf", "marina", "boathouse", "breakwater", "jetty", "tunnel",
        "arch", "fountain", "gazebo", "pavilion", "restaurant", "cinema",
        "planetarium", "yurt", "megalith", "stonehenge", "beacon",
        "bell cote", "mosque", "stupa", "toilet seat",
    ])),
    ("sports", _w([
        "ball", "baseball", "basketball", "football", "soccer ball",
        "rugby ball", "volleyball", "tennis ball", "ping-pong ball",
        "golf ball", "croquet ball", "racket", "barbell", "dumbbell",
        "bobsled", "parachute", "puck", "pool table", "punching bag",
        "ski", "snorkel", "canoe", "kayak", "paddle", "bow",
        "balance beam", "horizontal bar", "parallel bars", "vaulting horse",
        "pole", "javelin", "discus", "shot", "hammer", "trampoline",
        "scoreboard", "ballplayer", "golfer", "kite", "frisbee",
        "basketball backboard", "goal", "net", "referee", "cheerleader",
    ])),
    ("landscape_nature", _w([
        "alp", "cliff", "geyser", "lakeside", "promontory", "sandbar",
        "seashore", "valley", "volcano", "coral reef", "mountain",
        "canyon", "glacier", "iceberg", "waterfall", "rapids",
        "beacon",
    ])),
    ("text", _w([
        "book", "bookshelf", "bookcase", "magazine", "newspaper",
        "journal", "envelope", "notepad", "diary", "ledger", "manuscript",
        "parchment", "scroll", "menu", "map", "web site", "comic book",
        "crossword puzzle", "book jacket", "packet", "binder",
        "notebook", "monitor", "screen", "television", "laptop",
        "desktop computer", "hand-held computer", "iPod", "cellular telephone",
        "typewriter keyboard", "computer keyboard", "keypad", "space bar",
        "mouse", "printer", "photocopier", "scale", "rule", "slide rule",
        "abacus", "cash machine", "pay-phone", "dial telephone",
        "hard disc", "modem", "CD player", "loudspeaker", "microphone",
        "projector", "oscilloscope", "radio", "tape player",
    ])),
]

# 动物子类（按 ImageNet 标准排序的索引区间 + 兜底关键词）
ANIMAL_SUB_RANGES = {
    "dog": [(151, 268)],                       # 犬种
    "cat": [(281, 285)],                       # 家猫
    "bird": [(7, 24), (80, 100), (127, 146)],  # 鸟类三段
}

CATEGORY_DESC = {
    "animal": "动物", "food": "食物", "plant_flower": "植物花卉",
    "architecture": "建筑城市", "sports": "运动",
    "landscape_nature": "自然风景", "text": "文本截图",
    "vehicle": "车辆", "other": "其他",
}


def main():
    classes_path = os.path.join(HERE, "models", "imagenet_classes.txt")
    with open(classes_path, encoding="utf-8") as f:
        classes = [line.strip().split(" ", 1)[1] for line in f if line.strip()]

    mapping: dict[str, list[str]] = {}
    for idx, name in enumerate(classes):
        cat = OVERRIDES.get(name)
        if cat is None:
            cat = INDEX_OVERRIDES.get(idx)
        if cat is None:
            for c, pat in RULES:
                if pat.search(name):
                    cat = c
                    break
        if cat is None and idx <= 397:
            cat = "animal"  # ImageNet 前 398 类全为动物（索引兜底）
        if cat is None:
            cat = "other"
        mapping.setdefault(cat, []).append(name)

    # 动物子类
    animal_sub: dict[str, list[str]] = {"dog": [], "cat": [], "bird": []}
    for sub, ranges in ANIMAL_SUB_RANGES.items():
        for lo, hi in ranges:
            animal_sub[sub].extend(classes[lo:hi + 1])

    out = {
        "meta": {
            "version": "2.0",
            "description": "ImageNet-1k 细类 → 相册大类映射（整词正则 + 精确覆盖表）",
            "categories": sorted(mapping.keys()),
            "category_desc": CATEGORY_DESC,
            "total_classes": len(classes),
        },
        "mapping": mapping,
        "animal_sub": animal_sub,
    }
    out_path = os.path.join(HERE, "models", "imagenet_to_album.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    for cat, names in sorted(mapping.items()):
        print(f"{cat:18} {len(names)}")
    print(f"→ {out_path}")


if __name__ == "__main__":
    main()
