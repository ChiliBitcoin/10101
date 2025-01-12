import 'package:carousel_slider/carousel_slider.dart';
import 'package:flutter/material.dart';
import 'package:get_10101/common/color.dart';
import 'package:get_10101/features/welcome/seed_import_screen.dart';
import 'package:get_10101/features/welcome/welcome_screen.dart';
import 'package:go_router/go_router.dart';

final themeMode = ValueNotifier(2);

class CarouselItem {
  final String title;
  final String description;
  final String imagePath;
  final double aspectRatio;

  CarouselItem(this.title, this.description, this.imagePath, this.aspectRatio);
}

final List<CarouselItem> carouselItems = [
  CarouselItem("Your keys, your control", "Stay in control of your funds at all time.",
      "assets/carousel_1.jpg", 30 / 35),
  CarouselItem("Bitcoin only & Lightning fast.",
      "The highest level of security, at lightning speed.", "assets/carousel_2.jpg", 30 / 38),
  CarouselItem(
      "Perpetual futures trading.",
      "Experience P2P leveraged trading with no counterparty risk.",
      "assets/carousel_3.jpg",
      30 / 35),
  CarouselItem(
      "Hedging and synthetics",
      "You can now send, receive and hold USDP natively on Lightning.",
      "assets/carousel_4.jpg",
      30 / 28),
];

Widget carouselItemWidget(BuildContext context, CarouselItem item) {
  final baseHeight = MediaQuery.of(context).size.height * 0.45;
  final baseWidth = MediaQuery.of(context).size.width * 0.10;
  return Stack(children: [
    AspectRatio(
        aspectRatio: item.aspectRatio,
        child: Container(
            decoration: BoxDecoration(
                image: DecorationImage(
          fit: BoxFit.fitWidth,
          alignment: FractionalOffset.center,
          image: AssetImage(item.imagePath),
        )))),
    Padding(
      padding: EdgeInsets.only(left: baseWidth, right: baseWidth, top: baseHeight),
      child: Text(
        item.title,
        style: const TextStyle(fontSize: 30, fontWeight: FontWeight.bold),
        textAlign: TextAlign.center,
      ),
    ),
    Padding(
      padding: EdgeInsets.only(left: baseWidth, right: baseWidth, top: baseHeight + 100),
      child: Text(
        item.description,
        style: const TextStyle(fontSize: 18, color: Colors.black54),
        textAlign: TextAlign.center,
      ),
    )
  ]);
}

class Onboarding extends StatefulWidget {
  static const route = "/on-boarding";
  static const label = "Welcome";

  const Onboarding({Key? key}) : super(key: key);

  @override
  State<StatefulWidget> createState() {
    return _Onboarding();
  }
}

class _Onboarding extends State<Onboarding> {
  int _current = 0;
  final CarouselController _controller = CarouselController();

  @override
  Widget build(BuildContext context) {
    List<Widget> carouselItemWidgetLayers = [
      carouselItemWidget(context, carouselItems[0]),
      carouselItemWidget(context, carouselItems[1]),
      carouselItemWidget(context, carouselItems[2]),
      carouselItemWidget(context, carouselItems[3])
    ];

    return Scaffold(
        backgroundColor: Colors.white,
        body: SafeArea(
            child: Container(
          color: Colors.white,
          padding: const EdgeInsets.only(bottom: 20),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: <Widget>[
              SizedBox(
                height: MediaQuery.of(context).size.height * 0.70,
                child: CarouselSlider(
                  items: carouselItemWidgetLayers,
                  carouselController: _controller,
                  options: CarouselOptions(
                      viewportFraction: 1.0,
                      scrollDirection: Axis.horizontal,
                      autoPlay: false,
                      enlargeCenterPage: true,
                      aspectRatio: 15 / 22,
                      padEnds: true,
                      enableInfiniteScroll: false,
                      onPageChanged: (index, reason) {
                        setState(() {
                          _current = index;
                        });
                      }),
                ),
              ),
              Expanded(
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: carouselItemWidgetLayers.asMap().entries.map((entry) {
                    return GestureDetector(
                      onTap: () => _controller.animateToPage(entry.key),
                      child: Container(
                        width: 8.0,
                        height: 8.0,
                        margin: const EdgeInsets.symmetric(vertical: 8.0, horizontal: 4.0),
                        decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: (Theme.of(context).brightness == Brightness.dark
                                    ? Colors.white
                                    : Colors.black)
                                .withOpacity(_current == entry.key ? 0.6 : 0.2)),
                      ),
                    );
                  }).toList(),
                ),
              ),
              const SizedBox(height: 10),
              Column(children: [
                SizedBox(
                  width: 250,
                  child: ElevatedButton(
                      onPressed: () => GoRouter.of(context).go(WelcomeScreen.route),
                      style: ButtonStyle(
                        padding: MaterialStateProperty.all<EdgeInsets>(const EdgeInsets.all(15)),
                        backgroundColor: MaterialStateProperty.all<Color>(tenTenOnePurple),
                        shape: MaterialStateProperty.all<RoundedRectangleBorder>(
                          RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(40.0),
                            side: const BorderSide(color: tenTenOnePurple),
                          ),
                        ),
                      ),
                      child: const Wrap(
                        children: <Widget>[
                          Text(
                            "Create new wallet",
                            style: TextStyle(fontSize: 18, color: Colors.white),
                          ),
                        ],
                      )),
                ),
                const SizedBox(height: 5),
                SizedBox(
                  width: 250,
                  child: TextButton(
                    onPressed: () => GoRouter.of(context).go(SeedPhraseImporter.route),
                    style: ButtonStyle(
                      padding: MaterialStateProperty.all<EdgeInsets>(const EdgeInsets.all(15)),
                      backgroundColor: MaterialStateProperty.all<Color>(Colors.white),
                    ),
                    child: const Wrap(
                      children: <Widget>[
                        Text(
                          "Restore from backup",
                          style: TextStyle(
                            fontSize: 18,
                            color: Colors.black,
                            decoration: TextDecoration.underline,
                          ),
                        ),
                      ],
                    ),
                  ),
                )
              ]),
            ],
          ),
        )));
  }
}
